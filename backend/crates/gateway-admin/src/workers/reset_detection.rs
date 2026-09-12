//! 重置卡检测 Worker：读取实时配置、更新账号观测，并安全地自动消费重置卡。

use std::sync::{
    Arc, Mutex, PoisonError,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::Utc;
use futures::{StreamExt as _, future::BoxFuture, stream};
use gateway_core::{
    account::{AccountStatus, ProviderAccountId},
    lifecycle::CancellationToken,
    task::{ScheduledTask, WorkerCycleContext, WorkerTaskError},
};
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    AccountsService,
    model::{
        AdminError, AdminErrorKind, MutationActor, MutationContext, PageSize,
        accounts::{AccountListQuery, AccountPageItem, AccountRuntimeSnapshot},
        provider_credentials::{
            AccountDirectoryItem, ConsumeProviderResetCredit, ProviderResetCreditResult,
            ProviderResetCredits,
        },
        reset_detection::{
            CompleteResetCreditConsume, ResetCreditConsumeOutcome, ResetDetectionAccountScope,
            ResetDetectionSettings,
        },
    },
    ports::store::{AccountRuntimeStore, AccountStore, AdminStoreError, ResetDetectionStore},
};

/// 单轮最多同时访问四个账号，避免批量请求冲击上游。
pub const MAX_CONCURRENT_RESET_CHECKS: usize = 4;
/// 每个账号按稳定哈希分散到一秒窗口内，避免所有请求同时出发。
pub const RESET_CHECK_JITTER_MAX: Duration = Duration::from_secs(1);

/// Worker 使用的账号操作窄接口；生产实现严格转发到 [`AccountsService`]。
#[async_trait]
pub trait ResetDetectionAccountOperations: Send + Sync {
    async fn reset_credits(
        &self,
        context: &MutationContext,
        account_id: ProviderAccountId,
    ) -> Result<ProviderResetCredits, AdminError>;

    async fn consume_reset_credit(
        &self,
        context: &MutationContext,
        command: ConsumeProviderResetCredit,
    ) -> Result<ProviderResetCreditResult, AdminError>;

    async fn refresh_quota(
        &self,
        account_id: &ProviderAccountId,
    ) -> Result<AccountDirectoryItem, AdminError>;
}

struct AccountsServiceOperations {
    accounts: Arc<dyn AccountsService>,
}

#[async_trait]
impl ResetDetectionAccountOperations for AccountsServiceOperations {
    async fn reset_credits(
        &self,
        context: &MutationContext,
        account_id: ProviderAccountId,
    ) -> Result<ProviderResetCredits, AdminError> {
        self.accounts.reset_credits(context, account_id).await
    }

    async fn consume_reset_credit(
        &self,
        context: &MutationContext,
        command: ConsumeProviderResetCredit,
    ) -> Result<ProviderResetCreditResult, AdminError> {
        self.accounts.consume_reset_credit(context, command).await
    }

    async fn refresh_quota(
        &self,
        account_id: &ProviderAccountId,
    ) -> Result<AccountDirectoryItem, AdminError> {
        self.accounts.quota(account_id, true).await
    }
}

/// 一次 `run_cycle` 至多执行一轮重置卡检测。
pub struct ResetDetectionTask {
    reset_detection: Arc<dyn ResetDetectionStore>,
    accounts: Arc<dyn AccountStore>,
    account_runtime: Arc<dyn AccountRuntimeStore>,
    operations: Arc<dyn ResetDetectionAccountOperations>,
    last_round_started_at: Mutex<Option<Instant>>,
    round_in_progress: AtomicBool,
}

struct RoundGuard<'a> {
    in_progress: &'a AtomicBool,
}

impl Drop for RoundGuard<'_> {
    fn drop(&mut self) {
        self.in_progress.store(false, Ordering::Release);
    }
}

impl ResetDetectionTask {
    #[must_use]
    pub fn new(
        reset_detection: Arc<dyn ResetDetectionStore>,
        accounts: Arc<dyn AccountStore>,
        account_runtime: Arc<dyn AccountRuntimeStore>,
        account_service: Arc<dyn AccountsService>,
    ) -> Self {
        Self::new_with_operations(
            reset_detection,
            accounts,
            account_runtime,
            Arc::new(AccountsServiceOperations {
                accounts: account_service,
            }),
        )
    }

    #[must_use]
    pub fn new_with_operations(
        reset_detection: Arc<dyn ResetDetectionStore>,
        accounts: Arc<dyn AccountStore>,
        account_runtime: Arc<dyn AccountRuntimeStore>,
        operations: Arc<dyn ResetDetectionAccountOperations>,
    ) -> Self {
        Self {
            reset_detection,
            accounts,
            account_runtime,
            operations,
            last_round_started_at: Mutex::new(None),
            round_in_progress: AtomicBool::new(false),
        }
    }

    async fn cycle(&self, cancellation: &CancellationToken) -> Result<(), WorkerTaskError> {
        let settings = self
            .reset_detection
            .load_reset_detection_settings()
            .await
            .map_err(store_error)?;
        if !settings.enabled {
            return Ok(());
        }
        let Some(_round_guard) = self.round_due(&settings) else {
            return Ok(());
        };
        let runtime = self
            .account_runtime
            .active_rate_limits()
            .await
            .map_err(store_error)?;
        let targets = self.list_targets(&settings, runtime).await?;
        info!(targets = targets.len(), "重置卡检测轮次开始");
        let settings = &settings;
        stream::iter(targets.into_iter().map(|target| async move {
            self.process_account(target, settings, cancellation).await;
        }))
        .buffer_unordered(MAX_CONCURRENT_RESET_CHECKS)
        .collect::<Vec<_>>()
        .await;
        info!("重置卡检测轮次结束");
        Ok(())
    }

    async fn list_targets(
        &self,
        settings: &ResetDetectionSettings,
        runtime: AccountRuntimeSnapshot,
    ) -> Result<Vec<AccountPageItem>, WorkerTaskError> {
        let page_size = PageSize::new(PageSize::MAX)
            .map_err(|_| WorkerTaskError::safe("invalid reset detection page size"))?;
        let mut page_number = 1_u32;
        let mut targets = Vec::new();
        loop {
            let page = self
                .accounts
                .list_accounts(
                    AccountListQuery {
                        page: page_number,
                        page_size,
                        provider_kind: None,
                        group_filter: None,
                        search: None,
                        status: None,
                        sort: None,
                    },
                    runtime.clone(),
                )
                .await
                .map_err(store_error)?;
            let fetched = page.items.len();
            targets.extend(
                page.items
                    .into_iter()
                    .filter(|item| scope_includes(settings.account_scope, item.projection.status)),
            );
            let offset = u64::from(page_number).saturating_mul(u64::from(page_size.get()));
            if fetched == 0 || offset >= page.total {
                break;
            }
            page_number = page_number.saturating_add(1);
        }
        Ok(targets)
    }

    async fn process_account(
        &self,
        target: AccountPageItem,
        settings: &ResetDetectionSettings,
        cancellation: &CancellationToken,
    ) {
        let delay = account_jitter(&target.account.id);
        if !delay.is_zero() {
            tokio::select! {
                () = cancellation.cancelled() => return,
                () = tokio::time::sleep(delay) => {}
            }
        }
        if cancellation.is_cancelled() {
            return;
        }
        let Ok(account_id) = ProviderAccountId::new(target.account.id.clone()) else {
            warn!(
                account = target.account.id,
                "账号 ID 不合法，跳过重置卡检测"
            );
            return;
        };
        let context = system_context("reset_credit_check");
        let credits = match self
            .operations
            .reset_credits(&context, account_id.clone())
            .await
        {
            Ok(credits) => credits,
            Err(error) => {
                warn!(
                    account = %account_id,
                    error_kind = admin_error_kind(error.kind()),
                    "查询重置卡失败，跳过该账号"
                );
                return;
            }
        };
        let observed_at = Utc::now();
        if let Err(error) = self
            .reset_detection
            .record_reset_credits_observation(&account_id, credits.available_count, observed_at)
            .await
        {
            warn!(account = %account_id, %error, "写入重置卡观测失败，跳过自动消费");
            return;
        }
        if !settings.auto_consume_enabled
            || credits.available_count == 0
            || target.projection.status != AccountStatus::QuotaExhausted
            || target.account.provider_kind.as_str() != "openai"
            || cancellation.is_cancelled()
        {
            return;
        }
        self.consume_and_verify(account_id, cancellation).await;
    }

    async fn consume_and_verify(
        &self,
        account_id: ProviderAccountId,
        cancellation: &CancellationToken,
    ) {
        let reservation = match self
            .reset_detection
            .reserve_reset_credit_consume(&account_id, Uuid::new_v4())
            .await
        {
            Ok(reservation) => reservation,
            Err(error) => {
                warn!(account = %account_id, %error, "持久化自动消费幂等键失败，未发起消费");
                return;
            }
        };
        let redeem_request_id = reservation.redeem_request_id;
        if cancellation.is_cancelled() {
            return;
        }
        let context = MutationContext {
            actor: MutationActor::System,
            request_id: redeem_request_id.hyphenated().to_string(),
        };
        let result = self
            .operations
            .consume_reset_credit(
                &context,
                ConsumeProviderResetCredit {
                    account_id: account_id.clone(),
                    credit_id: None,
                    redeem_request_id,
                },
            )
            .await;
        let result = match result {
            Ok(result) => result,
            Err(error) if error.kind() == AdminErrorKind::UpstreamResultUnknown => {
                warn!(
                    account = %account_id,
                    %redeem_request_id,
                    resumed = reservation.resumed,
                    "自动消费结果未知，保留幂等键供下轮重放"
                );
                return;
            }
            Err(error) => {
                let error_kind = admin_error_kind(error.kind()).to_owned();
                warn!(
                    account = %account_id,
                    %redeem_request_id,
                    %error_kind,
                    "自动消费失败"
                );
                self.finish_consume(CompleteResetCreditConsume {
                    account_id,
                    redeem_request_id,
                    outcome: ResetCreditConsumeOutcome::Failed { error_kind },
                })
                .await;
                return;
            }
        };
        if !matches!(result.code.as_str(), "reset" | "already_redeemed") {
            warn!(
                account = %account_id,
                %redeem_request_id,
                provider_code = result.code,
                "上游未执行自动消费"
            );
            self.finish_consume(CompleteResetCreditConsume {
                account_id,
                redeem_request_id,
                outcome: ResetCreditConsumeOutcome::Rejected {
                    provider_code: result.code,
                },
            })
            .await;
            return;
        }
        let quota = match self.operations.refresh_quota(&account_id).await {
            Ok(quota) => quota,
            Err(error) => {
                warn!(
                    account = %account_id,
                    %redeem_request_id,
                    provider_code = result.code,
                    error_kind = admin_error_kind(error.kind()),
                    "自动消费已确认但额度复核失败，保留幂等键"
                );
                return;
            }
        };
        let quota_status = quota.projection.status;
        info!(
            account = %account_id,
            consumed_at = %Utc::now(),
            %redeem_request_id,
            provider_code = result.code,
            quota_status = quota_status.as_str(),
            "自动消费重置卡并完成额度复核"
        );
        if quota_status == AccountStatus::QuotaExhausted {
            warn!(
                account = %account_id,
                %redeem_request_id,
                "额度复核仍为耗尽，保留幂等键避免下轮消费新卡"
            );
            return;
        }
        self.finish_consume(CompleteResetCreditConsume {
            account_id,
            redeem_request_id,
            outcome: ResetCreditConsumeOutcome::Confirmed {
                provider_code: result.code,
                quota_status,
            },
        })
        .await;
    }

    async fn finish_consume(&self, completion: CompleteResetCreditConsume) {
        if let Err(error) = self
            .reset_detection
            .complete_reset_credit_consume(completion)
            .await
        {
            warn!(%error, "写入自动消费终态审计失败");
        }
    }

    fn round_due(&self, settings: &ResetDetectionSettings) -> Option<RoundGuard<'_>> {
        if self.round_in_progress.swap(true, Ordering::Acquire) {
            return None;
        }
        let interval = Duration::from_secs(u64::from(settings.poll_interval_secs.max(1)));
        let mut guard = self
            .last_round_started_at
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match *guard {
            Some(started_at) if started_at.elapsed() < interval => {
                self.round_in_progress.store(false, Ordering::Release);
                None
            }
            _ => {
                *guard = Some(Instant::now());
                Some(RoundGuard {
                    in_progress: &self.round_in_progress,
                })
            }
        }
    }
}

impl ScheduledTask for ResetDetectionTask {
    fn run_cycle(&self, context: WorkerCycleContext) -> BoxFuture<'_, Result<(), WorkerTaskError>> {
        let cancellation = context.cancellation().clone();
        Box::pin(async move { self.cycle(&cancellation).await })
    }
}

fn scope_includes(scope: ResetDetectionAccountScope, status: AccountStatus) -> bool {
    match scope {
        ResetDetectionAccountScope::AllNonError => matches!(
            status,
            AccountStatus::Normal | AccountStatus::QuotaExhausted | AccountStatus::RateLimited
        ),
        ResetDetectionAccountScope::Normal => status == AccountStatus::Normal,
        ResetDetectionAccountScope::Limited => {
            matches!(
                status,
                AccountStatus::QuotaExhausted | AccountStatus::RateLimited
            )
        }
    }
}

fn account_jitter(account_id: &str) -> Duration {
    let hash = account_id
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
        });
    let max_millis = u64::try_from(RESET_CHECK_JITTER_MAX.as_millis()).unwrap_or(u64::MAX);
    Duration::from_millis(hash % max_millis.saturating_add(1))
}

fn system_context(prefix: &str) -> MutationContext {
    MutationContext {
        actor: MutationActor::System,
        request_id: format!("{prefix}_{}", Uuid::now_v7().simple()),
    }
}

const fn admin_error_kind(kind: AdminErrorKind) -> &'static str {
    match kind {
        AdminErrorKind::Invalid => "invalid",
        AdminErrorKind::Unauthorized => "unauthorized",
        AdminErrorKind::NotFound => "not_found",
        AdminErrorKind::Conflict => "conflict",
        AdminErrorKind::RateLimited => "rate_limited",
        AdminErrorKind::BadGateway => "bad_gateway",
        AdminErrorKind::UpstreamResultUnknown => "upstream_result_unknown",
        AdminErrorKind::Unavailable => "unavailable",
        AdminErrorKind::Internal => "internal",
    }
}

fn store_error(error: AdminStoreError) -> WorkerTaskError {
    WorkerTaskError::safe(error.to_string())
}
