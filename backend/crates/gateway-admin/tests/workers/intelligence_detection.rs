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
        IntelligenceDetectionTask, extract_html_document, matched_phrases, prompt_for_round,
    },
};
use gateway_core::{
    engine::probe::{AccountProbe, AccountProbeRequest, AccountProbeResult},
    lifecycle::CancellationToken,
    routing::{ConfigRevision, ProviderKind},
    runtime::SnapshotControl,
    task::{ScheduledTask, WorkerCycleContext, WorkerId, WorkerKind},
};
use uuid::Uuid;

use crate::use_case::accounts::{EventLog, FakeAccountStore, FakeProviderAdmin, account_record};

#[test]
fn matches_degraded_phrases_after_whitespace_normalization() {
    for text in [
        "内嵌SVG和CSS",
        "内嵌 SVG 和 CSS",
        "内联SVG",
        "内联 SVG 和 CSS",
        "使用内嵌的SVG",
        "inline SVG",
        "SVG and CSS",
        "用内联样式写SVG动画",
    ] {
        assert!(
            !matched_phrases(text).is_empty(),
            "expected degraded phrase to match: {text}"
        );
    }
    for text in [
        "<svg viewBox=\"0 0 10 10\"></svg>",
        "这个 CSS 不要用内联样式",
        "一切正常的 HTML 动画。",
    ] {
        assert!(
            matched_phrases(text).is_empty(),
            "expected normal response not to match: {text}"
        );
    }
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
    latest_checked_at: Arc<Mutex<Option<chrono::DateTime<Utc>>>>,
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
            latest_checked_at: Arc::new(Mutex::new(None)),
            fail_account: None,
        })
    }

    fn fail_account(&mut self, account_id: &str) {
        self.fail_account = Some(account_id.to_owned());
    }

    fn records(&self) -> Vec<NewDetectionRecord> {
        self.records.lock().expect("detection records").clone()
    }

    fn set_latest_checked_at(&self, checked_at: chrono::DateTime<Utc>) {
        *self
            .latest_checked_at
            .lock()
            .expect("latest detection timestamp") = Some(checked_at);
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
        Ok(*self
            .latest_checked_at
            .lock()
            .expect("latest detection timestamp"))
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
        *self
            .latest_checked_at
            .lock()
            .expect("latest detection timestamp") = Some(Utc::now());
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
                reasoning: Vec::new(),
            })
        })
    }
}

#[test]
fn prompt_for_round_is_stable_and_covers_all_animals() {
    let first = Uuid::from_bytes([0; 16]);
    assert_eq!(prompt_for_round(first), prompt_for_round(first));
    let prompts = (0_u8..=u8::MAX)
        .map(|first_byte| prompt_for_round(Uuid::from_bytes([first_byte; 16])))
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(prompts.len(), 12);
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

#[tokio::test]
async fn recent_database_round_blocks_new_instance_cycle() {
    let detection = DetectionFixture::new(targets(1));
    detection.set_latest_checked_at(Utc::now());
    let probe = Arc::new(CountingProbe {
        active: AtomicUsize::new(0),
        peak: AtomicUsize::new(0),
    });
    run_detection(detection.clone(), probe.clone(), 1).await;

    assert!(detection.records().is_empty());
    assert_eq!(probe.peak(), 0);
}
