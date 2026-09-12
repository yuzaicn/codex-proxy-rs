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
        IntelligenceDetectionTask, extract_html_document, matched_phrases,
    },
};
use gateway_core::{
    engine::probe::{AccountProbe, AccountProbeRequest, AccountProbeResult},
    lifecycle::CancellationToken,
    routing::{ConfigRevision, ProviderKind},
    runtime::SnapshotControl,
    task::{ScheduledTask, WorkerCycleContext, WorkerId, WorkerKind},
};

use crate::use_case::accounts::{EventLog, FakeAccountStore, FakeProviderAdmin, account_record};

#[test]
fn matches_degraded_phrases_case_insensitively() {
    let text = "这里是内嵌 SVG 和 CSS 的说明，还提到了 Inline svg。";
    let matched = matched_phrases(text);
    assert_eq!(matched, vec!["内嵌 SVG 和 CSS", "inline SVG"]);
    assert!(matched_phrases("一切正常的 HTML 动画。").is_empty());
}

#[test]
fn extracts_fenced_html_block_first() {
    let text = "说明\n```html\n<!DOCTYPE html><html><body>ok</body></html>\n```\n结尾";
    assert_eq!(
        extract_html_document(text),
        "<!DOCTYPE html><html><body>ok</body></html>"
    );
}

#[test]
fn extracts_bare_html_document() {
    let text = "前置说明 <HTML><body>x</body></HTML> 之后的内容";
    assert_eq!(extract_html_document(text), "<HTML><body>x</body></HTML>");
}

#[test]
fn keeps_full_text_without_html_structure() {
    let text = "纯文字答复，没有任何标记。";
    assert_eq!(extract_html_document(text), text);
}

#[derive(Clone)]
struct DetectionFixture {
    config: DetectionConfig,
    targets: Vec<DetectionTarget>,
    records: Arc<Mutex<Vec<NewDetectionRecord>>>,
    fail_account: Option<String>,
}

impl DetectionFixture {
    fn new(targets: Vec<DetectionTarget>) -> Arc<Self> {
        Arc::new(Self {
            config: DetectionConfig {
                enabled: true,
                account_scope: DetectionAccountScope::AllAccounts,
                interval_secs: 3600,
                model: "test-model".to_owned(),
                updated_at: Utc::now(),
            },
            targets,
            records: Arc::new(Mutex::new(Vec::new())),
            fail_account: None,
        })
    }

    fn fail_account(&mut self, account_id: &str) {
        self.fail_account = Some(account_id.to_owned());
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

    async fn list_detection_targets(
        &self,
        _: &DetectionAccountScope,
    ) -> AdminStoreResult<Vec<DetectionTarget>> {
        Ok(self.targets.clone())
    }

    async fn load_detection_record_html(&self, _: i64) -> AdminStoreResult<Option<String>> {
        Ok(None)
    }

    async fn insert_detection_record(&self, record: NewDetectionRecord) -> AdminStoreResult<()> {
        if self.fail_account.as_deref() == Some(record.account_id.as_str()) {
            return Err(store_error("detection records"));
        }
        self.records.lock().expect("detection records").push(record);
        Ok(())
    }
}

struct CountingProbe {
    active: AtomicUsize,
    peak: AtomicUsize,
}

impl CountingProbe {
    fn peak(&self) -> usize {
        self.peak.load(Ordering::Acquire)
    }
}

impl AccountProbe for CountingProbe {
    fn probe(
        &self,
        _: AccountProbeRequest,
    ) -> BoxFuture<'_, Result<AccountProbeResult, gateway_core::engine::probe::AccountProbeError>>
    {
        Box::pin(async move {
            let active = self.active.fetch_add(1, Ordering::AcqRel) + 1;
            self.peak.fetch_max(active, Ordering::AcqRel);
            tokio::time::sleep(Duration::from_millis(5)).await;
            self.active.fetch_sub(1, Ordering::AcqRel);
            Ok(AccountProbeResult {
                text: vec!["normal response".to_owned()],
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

async fn run_detection(detection: Arc<DetectionFixture>, probe: Arc<CountingProbe>, count: usize) {
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
async fn detection_round_records_share_one_round_id() {
    let detection = DetectionFixture::new(targets(4));
    let probe = Arc::new(CountingProbe {
        active: AtomicUsize::new(0),
        peak: AtomicUsize::new(0),
    });
    run_detection(detection.clone(), probe, 4).await;

    let records = detection.records();
    assert_eq!(records.len(), 4);
    assert!(
        records
            .iter()
            .all(|record| record.detection_round_id == records[0].detection_round_id)
    );
}

#[tokio::test]
async fn detection_record_failure_does_not_abort_other_accounts() {
    let mut detection = DetectionFixture::new(targets(3));
    Arc::get_mut(&mut detection)
        .expect("unique detection fixture")
        .fail_account("acct_1");
    let probe = Arc::new(CountingProbe {
        active: AtomicUsize::new(0),
        peak: AtomicUsize::new(0),
    });
    run_detection(detection.clone(), probe, 3).await;

    let records = detection.records();
    assert_eq!(records.len(), 2);
    assert!(records.iter().all(|record| record.account_id != "acct_1"));
}

#[tokio::test]
async fn detection_probe_concurrency_is_bounded() {
    let detection = DetectionFixture::new(targets(32));
    let probe = Arc::new(CountingProbe {
        active: AtomicUsize::new(0),
        peak: AtomicUsize::new(0),
    });
    run_detection(detection, probe.clone(), 32).await;

    assert_eq!(probe.peak(), 16);
}
