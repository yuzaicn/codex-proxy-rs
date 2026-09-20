use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use futures::future::BoxFuture;
use gateway_admin::{
    model::{
        MutationContext,
        detection::{
            DetectionAccountScope, DetectionConfig, DetectionConfigMutation, DetectionRecord,
            DetectionRecordQuery, DetectionRound, DetectionTarget, NewDetectionRecord,
            ReplaceDetectionConfig,
        },
    },
    ports::{
        provider::{ProviderAdmin, ProviderAdminRegistry},
        store::{AdminStoreError, AdminStoreErrorKind, AdminStoreResult, DetectionStore},
    },
    workers::intelligence_detection::{
        IntelligenceDetectionTask, should_suspend_degraded, verdict_for_margin,
    },
};
use gateway_core::{
    engine::probe::{
        AccountProbe, AccountProbeError, AccountProbeErrorSource, AccountProbeRequest,
        AccountProbeResult,
    },
    error::{GatewayError, GatewayErrorKind},
    lifecycle::CancellationToken,
    routing::{ConfigRevision, ProviderKind},
    runtime::SnapshotControl,
    task::{ScheduledTask, WorkerCycleContext, WorkerId, WorkerKind},
};
use serde_json::Value;

use crate::use_case::accounts::{EventLog, FakeAccountStore, FakeProviderAdmin, account_record};

#[test]
fn margin_dead_zone_has_stable_three_state_boundaries() {
    assert_eq!(verdict_for_margin(0.2), "normal");
    assert_eq!(verdict_for_margin(-0.2), "degraded");
    assert_eq!(verdict_for_margin(0.199_999), "undetermined");
    assert_eq!(verdict_for_margin(-0.199_999), "undetermined");
}

#[test]
fn enforcement_requires_two_actionable_degraded_rounds() {
    assert!(!should_suspend_degraded("shadow", Some("degraded"), false));
    assert!(!should_suspend_degraded("enforce", None, false));
    assert!(!should_suspend_degraded(
        "enforce",
        Some("undetermined"),
        false
    ));
    assert!(!should_suspend_degraded(
        "enforce",
        Some("unverifiable"),
        false
    ));
    assert!(should_suspend_degraded("enforce", Some("degraded"), false));
    assert!(!should_suspend_degraded("enforce", Some("degraded"), true));
}

#[derive(Clone)]
struct DetectionFixture {
    config: DetectionConfig,
    targets: Vec<DetectionTarget>,
    records: Arc<Mutex<Vec<NewDetectionRecord>>>,
    latest_checked_at: Arc<Mutex<Option<chrono::DateTime<Utc>>>>,
}

impl DetectionFixture {
    fn new(targets: Vec<DetectionTarget>) -> Arc<Self> {
        Arc::new(Self {
            config: DetectionConfig {
                enabled: true,
                account_scope: DetectionAccountScope::AllAccounts,
                interval_secs: 3600,
                model: "gpt-5.4".to_owned(),
                reasoning_effort: "auto".to_owned(),
                enforcement: "shadow".to_owned(),
                updated_at: Utc::now(),
            },
            targets,
            records: Arc::new(Mutex::new(Vec::new())),
            latest_checked_at: Arc::new(Mutex::new(None)),
        })
    }

    fn records(&self) -> Vec<NewDetectionRecord> {
        self.records.lock().expect("detection records").clone()
    }
}

fn store_error(resource: &'static str) -> AdminStoreError {
    AdminStoreError::new(AdminStoreErrorKind::Unavailable, resource, "test failure")
}

#[async_trait]
impl DetectionStore for DetectionFixture {
    async fn load_detection_config(&self) -> AdminStoreResult<Option<DetectionConfig>> {
        Ok(Some(self.config.clone()))
    }

    async fn replace_detection_config(
        &self,
        _: ReplaceDetectionConfig,
        _: &MutationContext,
    ) -> AdminStoreResult<DetectionConfigMutation> {
        Err(store_error("detection config"))
    }

    async fn list_detection_records(
        &self,
        _: DetectionRecordQuery,
    ) -> AdminStoreResult<Vec<DetectionRecord>> {
        Err(store_error("detection records"))
    }

    async fn list_detection_rounds(&self, _: u32) -> AdminStoreResult<Vec<DetectionRound>> {
        Err(store_error("detection rounds"))
    }

    async fn latest_detection_checked_at(&self) -> AdminStoreResult<Option<chrono::DateTime<Utc>>> {
        Ok(*self.latest_checked_at.lock().expect("latest timestamp"))
    }

    async fn list_detection_targets(
        &self,
        _: &DetectionAccountScope,
    ) -> AdminStoreResult<Vec<DetectionTarget>> {
        Ok(self.targets.clone())
    }

    async fn latest_actionable_detection_verdict(
        &self,
        account_id: &str,
    ) -> AdminStoreResult<Option<String>> {
        Ok(self
            .records
            .lock()
            .expect("detection records")
            .iter()
            .rev()
            .find(|record| {
                record.account_id == account_id
                    && matches!(record.verdict.as_str(), "normal" | "degraded")
            })
            .map(|record| record.verdict.clone()))
    }

    async fn load_detection_record_html(&self, _: i64) -> AdminStoreResult<Option<String>> {
        Ok(None)
    }

    async fn insert_detection_record(&self, record: NewDetectionRecord) -> AdminStoreResult<()> {
        self.records.lock().expect("detection records").push(record);
        *self.latest_checked_at.lock().expect("latest timestamp") = Some(Utc::now());
        Ok(())
    }
}

struct ProbeFixture {
    active: AtomicUsize,
    peak: AtomicUsize,
    calls: AtomicUsize,
    fail_first: usize,
    bodies: Mutex<Vec<Value>>,
}

impl AccountProbe for ProbeFixture {
    fn probe(
        &self,
        request: AccountProbeRequest,
    ) -> BoxFuture<'_, Result<AccountProbeResult, AccountProbeError>> {
        Box::pin(async move {
            let AccountProbeRequest { operation, .. } = request;
            let body = match operation {
                gateway_core::operation::Operation::Generate(generate) => {
                    generate.protocol_payload().body().clone()
                }
                _ => panic!("fingerprint probe must use generate"),
            };
            self.bodies
                .lock()
                .expect("probe bodies")
                .push(Value::Object(body));
            let call = self.calls.fetch_add(1, Ordering::AcqRel);
            if call < self.fail_first {
                return Err(AccountProbeError::new(
                    GatewayError::new(GatewayErrorKind::UpstreamUnavailable, "probe failed"),
                    AccountProbeErrorSource::Upstream,
                    None,
                    None,
                ));
            }
            let active = self.active.fetch_add(1, Ordering::AcqRel) + 1;
            self.peak.fetch_max(active, Ordering::AcqRel);
            tokio::time::sleep(Duration::from_millis(2)).await;
            self.active.fetch_sub(1, Ordering::AcqRel);
            Ok(AccountProbeResult {
                text: vec!["1 2 3 4 5".to_owned()],
                reasoning: Vec::new(),
            })
        })
    }
}

struct NoopSnapshot;

impl SnapshotControl for NoopSnapshot {
    fn publish_committed(&self, _: ConfigRevision) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

fn targets(count: usize) -> Vec<DetectionTarget> {
    (0..count)
        .map(|index| DetectionTarget {
            account_id: format!("acct_{index}"),
            provider_kind: ProviderKind::new("test").expect("provider kind"),
            scheduling_suspended: false,
            scheduling_suspended_by: None,
        })
        .collect()
}

fn account_store(count: usize, events: &EventLog) -> Arc<FakeAccountStore> {
    let store = FakeAccountStore::new("test", events.clone());
    let accounts = (0..count)
        .map(|index| {
            let mut account = account_record("test");
            account.id = format!("acct_{index}");
            account
        })
        .collect();
    store.set_accounts(accounts);
    store
}

async fn run_detection(detection: Arc<DetectionFixture>, probe: Arc<ProbeFixture>, count: usize) {
    let events: EventLog = Arc::new(Mutex::new(Vec::new()));
    let accounts = account_store(count, &events);
    let provider = FakeProviderAdmin::new("test", events);
    let registry = ProviderAdminRegistry::new([provider as Arc<dyn ProviderAdmin>])
        .expect("provider registry");
    let task = IntelligenceDetectionTask::new(
        detection,
        accounts,
        registry,
        probe,
        Arc::new(NoopSnapshot),
    );
    let worker = WorkerId::try_new(WorkerKind::IntelligenceDetection, "test").expect("worker id");
    task.run_cycle(WorkerCycleContext::new(
        worker,
        None,
        CancellationToken::new(),
    ))
    .await
    .expect("detection cycle");
}

#[tokio::test]
async fn three_fixed_challenges_are_sent_without_reasoning_and_recorded_once() {
    let detection = DetectionFixture::new(targets(1));
    let probe = Arc::new(ProbeFixture {
        active: AtomicUsize::new(0),
        peak: AtomicUsize::new(0),
        calls: AtomicUsize::new(0),
        fail_first: 0,
        bodies: Mutex::new(Vec::new()),
    });
    run_detection(detection.clone(), probe.clone(), 1).await;
    assert_eq!(probe.calls.load(Ordering::Acquire), 3);
    assert!(
        probe
            .bodies
            .lock()
            .expect("probe bodies")
            .iter()
            .all(|body| body.get("reasoning").is_none())
    );
    let records = detection.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].detector, "fingerprint");
    assert_eq!(
        records[0]
            .probe_payload
            .as_ref()
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(3)
    );
}

#[tokio::test]
async fn partial_failures_and_zero_success_are_unverifiable() {
    let detection = DetectionFixture::new(targets(2));
    let probe = Arc::new(ProbeFixture {
        active: AtomicUsize::new(0),
        peak: AtomicUsize::new(0),
        calls: AtomicUsize::new(0),
        fail_first: 4,
        bodies: Mutex::new(Vec::new()),
    });
    run_detection(detection.clone(), probe, 2).await;
    let records = detection.records();
    assert_eq!(records.len(), 2);
    assert!(
        records
            .iter()
            .all(|record| record.verdict == "unverifiable")
    );
    assert!(records.iter().any(|record| record.margin.is_some()));
    assert!(records.iter().any(|record| record.margin.is_none()));
}

#[tokio::test]
async fn account_concurrency_is_six_with_three_requests_each() {
    let detection = DetectionFixture::new(targets(32));
    let probe = Arc::new(ProbeFixture {
        active: AtomicUsize::new(0),
        peak: AtomicUsize::new(0),
        calls: AtomicUsize::new(0),
        fail_first: 0,
        bodies: Mutex::new(Vec::new()),
    });
    run_detection(detection, probe.clone(), 32).await;
    assert_eq!(probe.calls.load(Ordering::Acquire), 96);
    assert!(probe.peak.load(Ordering::Acquire) <= 18);
}
