use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
    time::SystemTime,
};

use async_trait::async_trait;
use chrono::{TimeDelta, Utc};
use gateway_admin::{
    model::{
        AdminError, AdminErrorKind, MutationContext,
        accounts::{AccountRuntimeSnapshot, AccountStatus},
        provider_credentials::{
            AccountDirectoryItem, ConsumeProviderResetCredit, ProviderQuota,
            ProviderResetCreditResult, ProviderResetCredits,
        },
        reset_detection::{
            CompleteResetCreditConsume, ReplaceResetDetectionSettings, ResetCreditConsumeOutcome,
            ResetCreditConsumeReservation, ResetDetectionAccountScope, ResetDetectionSettings,
            ResetDetectionSettingsMutation,
        },
    },
    ports::store::{
        AccountRuntimeStore, AdminStoreError, AdminStoreErrorKind, AdminStoreResult,
        ResetDetectionStore,
    },
    workers::reset_detection::{ResetDetectionAccountOperations, ResetDetectionTask},
};
use gateway_core::{
    account::{AccountStatusProjection, ProviderAccountId, QuotaEvidence, QuotaState},
    lifecycle::CancellationToken,
    task::{ScheduledTask, WorkerCycleContext, WorkerId, WorkerKind},
};
use uuid::Uuid;

use crate::use_case::accounts::{EventLog, FakeAccountStore, account_record};

#[derive(Clone)]
struct ResetStoreFixture {
    settings: ResetDetectionSettings,
    observations: Arc<Mutex<Vec<(String, u64)>>>,
    pending: Arc<Mutex<BTreeMap<String, Uuid>>>,
    completions: Arc<Mutex<Vec<CompleteResetCreditConsume>>>,
}

impl ResetStoreFixture {
    fn new(
        enabled: bool,
        auto_consume_enabled: bool,
        scope: ResetDetectionAccountScope,
    ) -> Arc<Self> {
        Arc::new(Self {
            settings: ResetDetectionSettings {
                enabled,
                poll_interval_secs: 30,
                account_scope: scope,
                auto_consume_enabled,
                updated_at: Utc::now(),
                config_revision: gateway_admin::model::Revision::new(1).expect("revision"),
            },
            observations: Arc::new(Mutex::new(Vec::new())),
            pending: Arc::new(Mutex::new(BTreeMap::new())),
            completions: Arc::new(Mutex::new(Vec::new())),
        })
    }
}

#[async_trait]
impl ResetDetectionStore for ResetStoreFixture {
    async fn load_reset_detection_settings(&self) -> AdminStoreResult<ResetDetectionSettings> {
        Ok(self.settings.clone())
    }

    async fn replace_reset_detection_settings(
        &self,
        _: ReplaceResetDetectionSettings,
        _: &MutationContext,
    ) -> AdminStoreResult<ResetDetectionSettingsMutation> {
        Err(store_error())
    }

    async fn record_reset_credits_observation(
        &self,
        account_id: &ProviderAccountId,
        available_count: u64,
        _: chrono::DateTime<Utc>,
    ) -> AdminStoreResult<()> {
        self.observations
            .lock()
            .expect("observations")
            .push((account_id.as_str().to_owned(), available_count));
        Ok(())
    }

    async fn reserve_reset_credit_consume(
        &self,
        account_id: &ProviderAccountId,
        candidate: Uuid,
    ) -> AdminStoreResult<ResetCreditConsumeReservation> {
        let mut pending = self.pending.lock().expect("pending consumes");
        let resumed = pending.contains_key(account_id.as_str());
        let redeem_request_id = *pending
            .entry(account_id.as_str().to_owned())
            .or_insert(candidate);
        Ok(ResetCreditConsumeReservation {
            redeem_request_id,
            resumed,
        })
    }

    async fn complete_reset_credit_consume(
        &self,
        completion: CompleteResetCreditConsume,
    ) -> AdminStoreResult<()> {
        self.pending
            .lock()
            .expect("pending consumes")
            .remove(completion.account_id.as_str());
        self.completions
            .lock()
            .expect("consume completions")
            .push(completion);
        Ok(())
    }
}

struct RuntimeFixture {
    snapshot: AccountRuntimeSnapshot,
}

#[async_trait]
impl AccountRuntimeStore for RuntimeFixture {
    async fn active_rate_limits(&self) -> AdminStoreResult<AccountRuntimeSnapshot> {
        Ok(self.snapshot.clone())
    }

    async fn account_runtime(&self, _: &[String]) -> AdminStoreResult<AccountRuntimeSnapshot> {
        Ok(self.snapshot.clone())
    }
}

struct OperationsFixture {
    available_count: u64,
    reset_reads: Mutex<usize>,
    consumes: Mutex<Vec<ConsumeProviderResetCredit>>,
    consume_results: Mutex<VecDeque<Result<ProviderResetCreditResult, AdminError>>>,
    refreshes: Mutex<usize>,
    refreshed_status: AccountStatus,
}

impl OperationsFixture {
    fn new(
        available_count: u64,
        consume_results: Vec<Result<ProviderResetCreditResult, AdminError>>,
        refreshed_status: AccountStatus,
    ) -> Arc<Self> {
        Arc::new(Self {
            available_count,
            reset_reads: Mutex::new(0),
            consumes: Mutex::new(Vec::new()),
            consume_results: Mutex::new(consume_results.into()),
            refreshes: Mutex::new(0),
            refreshed_status,
        })
    }
}

#[async_trait]
impl ResetDetectionAccountOperations for OperationsFixture {
    async fn reset_credits(
        &self,
        _: &MutationContext,
        _: ProviderAccountId,
    ) -> Result<ProviderResetCredits, AdminError> {
        *self.reset_reads.lock().expect("reset reads") += 1;
        Ok(ProviderResetCredits {
            available_count: self.available_count,
            credits: Vec::new(),
        })
    }

    async fn consume_reset_credit(
        &self,
        _: &MutationContext,
        command: ConsumeProviderResetCredit,
    ) -> Result<ProviderResetCreditResult, AdminError> {
        self.consumes.lock().expect("consumes").push(command);
        self.consume_results
            .lock()
            .expect("consume results")
            .pop_front()
            .expect("configured consume result")
    }

    async fn refresh_quota(
        &self,
        _: &ProviderAccountId,
    ) -> Result<AccountDirectoryItem, AdminError> {
        *self.refreshes.lock().expect("refreshes") += 1;
        Ok(directory_item(self.refreshed_status))
    }
}

fn directory_item(status: AccountStatus) -> AccountDirectoryItem {
    AccountDirectoryItem {
        account: account_record("openai"),
        plan_type_display: None,
        projection: AccountStatusProjection {
            status,
            error_reason: None,
            error_message: None,
            rate_limited_until: None,
        },
        usage: None,
        quota: ProviderQuota {
            plan_type: None,
            observed_at: Some(Utc::now()),
            refresh_token_expires_at: None,
            windows: Vec::new(),
            limit_reached: status == AccountStatus::QuotaExhausted,
            provider_data: None,
        },
    }
}

fn account_store(account: gateway_admin::model::accounts::AccountRecord) -> Arc<FakeAccountStore> {
    let event_log: EventLog = Arc::new(Mutex::new(Vec::new()));
    let store = FakeAccountStore::new("openai", event_log);
    store.set_accounts(vec![account]);
    store
}

fn runtime(snapshot: AccountRuntimeSnapshot) -> Arc<dyn AccountRuntimeStore> {
    Arc::new(RuntimeFixture { snapshot })
}

async fn run_task(
    store: Arc<ResetStoreFixture>,
    accounts: Arc<FakeAccountStore>,
    runtime: Arc<dyn AccountRuntimeStore>,
    operations: Arc<OperationsFixture>,
) {
    let task = ResetDetectionTask::new_with_operations(store, accounts, runtime, operations);
    let worker = WorkerId::try_new(WorkerKind::ResetDetection, "test").expect("worker ID");
    task.run_cycle(WorkerCycleContext::new(
        worker,
        None,
        CancellationToken::new(),
    ))
    .await
    .expect("reset detection cycle");
}

fn exhausted_account() -> gateway_admin::model::accounts::AccountRecord {
    let mut account = account_record("openai");
    account.quota =
        QuotaState::exhausted(QuotaEvidence::UsageLimitReached, SystemTime::now(), None);
    account
}

fn success(code: &str) -> Result<ProviderResetCreditResult, AdminError> {
    Ok(ProviderResetCreditResult {
        code: code.to_owned(),
        credit: None,
    })
}

#[tokio::test]
async fn ambiguous_result_reuses_redeem_request_id_after_restart() {
    let store = ResetStoreFixture::new(true, true, ResetDetectionAccountScope::Limited);
    let accounts = account_store(exhausted_account());
    let runtime = runtime(AccountRuntimeSnapshot::default());
    let operations = OperationsFixture::new(
        1,
        vec![
            Err(AdminError::new(
                AdminErrorKind::UpstreamResultUnknown,
                "unknown",
            )),
            success("already_redeemed"),
        ],
        AccountStatus::Normal,
    );

    run_task(
        store.clone(),
        accounts.clone(),
        runtime.clone(),
        operations.clone(),
    )
    .await;
    run_task(store.clone(), accounts, runtime, operations.clone()).await;

    let consumes = operations.consumes.lock().expect("consumes");
    assert_eq!(consumes.len(), 2);
    assert_eq!(consumes[0].redeem_request_id, consumes[1].redeem_request_id);
    assert_eq!(store.completions.lock().expect("completions").len(), 1);
}

#[tokio::test]
async fn rate_limited_without_quota_exhaustion_does_not_consume() {
    let store = ResetStoreFixture::new(true, true, ResetDetectionAccountScope::Limited);
    let account = account_record("openai");
    let account_id = account.id.clone();
    let accounts = account_store(account);
    let mut snapshot = AccountRuntimeSnapshot::default();
    snapshot
        .rate_limited_until
        .insert(account_id, Utc::now() + TimeDelta::minutes(5));
    let operations = OperationsFixture::new(1, Vec::new(), AccountStatus::Normal);

    run_task(
        store.clone(),
        accounts,
        runtime(snapshot),
        operations.clone(),
    )
    .await;

    assert_eq!(*operations.reset_reads.lock().expect("reset reads"), 1);
    assert!(operations.consumes.lock().expect("consumes").is_empty());
    assert_eq!(store.observations.lock().expect("observations").len(), 1);
}

#[tokio::test]
async fn confirmed_consume_triggers_fresh_quota_verification() {
    let store = ResetStoreFixture::new(true, true, ResetDetectionAccountScope::Limited);
    let operations = OperationsFixture::new(1, vec![success("reset")], AccountStatus::Normal);

    run_task(
        store.clone(),
        account_store(exhausted_account()),
        runtime(AccountRuntimeSnapshot::default()),
        operations.clone(),
    )
    .await;

    assert_eq!(*operations.refreshes.lock().expect("refreshes"), 1);
    let completions = store.completions.lock().expect("completions");
    assert!(matches!(
        completions[0].outcome,
        ResetCreditConsumeOutcome::Confirmed {
            quota_status: AccountStatus::Normal,
            ..
        }
    ));
}

#[tokio::test]
async fn disabled_worker_cycle_does_not_read_accounts_or_upstream() {
    let store = ResetStoreFixture::new(false, true, ResetDetectionAccountScope::AllNonError);
    let event_log: EventLog = Arc::new(Mutex::new(Vec::new()));
    let accounts = FakeAccountStore::new("openai", event_log.clone());
    let operations = OperationsFixture::new(1, Vec::new(), AccountStatus::Normal);

    run_task(
        store,
        accounts,
        runtime(AccountRuntimeSnapshot::default()),
        operations.clone(),
    )
    .await;

    assert!(event_log.lock().expect("events").is_empty());
    assert_eq!(*operations.reset_reads.lock().expect("reset reads"), 0);
    assert!(operations.consumes.lock().expect("consumes").is_empty());
}

fn store_error() -> AdminStoreError {
    AdminStoreError::new(
        AdminStoreErrorKind::Unavailable,
        "reset detection",
        "test failure",
    )
}
