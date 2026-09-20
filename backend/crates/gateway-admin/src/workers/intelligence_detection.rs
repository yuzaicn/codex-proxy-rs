//! ModelTrace 指纹检测 Worker。
//!
//! 每个账号每轮执行三个固定挑战，收齐后聚合评分并落一条记录。`shadow` 模式记录
//! 全部结论但不新建暂停，`enforce` 模式仅在连续两轮 `degraded` 后暂停账号；单轮
//! `normal` 仍可释放检测来源的既有暂停。评分库加载失败或证据不足都不会猜测结论。

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures::future::BoxFuture;
use futures::stream::{self, StreamExt};
use gateway_core::{
    account::{ProviderAccountId, SchedulingSuspensionSource},
    engine::probe::{AccountProbe, AccountProbeError, AccountProbeRequest, AccountProbeResult},
    lifecycle::CancellationToken,
    routing::{ConfigRevision, UpstreamModelId},
    runtime::SnapshotControl,
    task::{ScheduledTask, WorkerCycleContext, WorkerTaskError},
};
use model_fingerprint::{BANK_ID, CHALLENGES, FingerprintChallenge, FingerprintScorer};
use serde_json::{Value, json};
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    model::{
        MutationActor, MutationContext, Revision,
        detection::{DetectionConfig, DetectionTarget, NewDetectionRecord},
    },
    ports::{
        provider::{ProviderAdmin, ProviderAdminError, ProviderAdminRegistry},
        store::{AccountStore, AdminStoreError, DetectionStore},
    },
};

const MAX_CONCURRENT_PROBES: usize = 6;
const MARGIN_THRESHOLD: f64 = 0.2;
const VERDICT_NORMAL: &str = "normal";
const VERDICT_DEGRADED: &str = "degraded";
const VERDICT_UNDETERMINED: &str = "undetermined";
const VERDICT_UNVERIFIABLE: &str = "unverifiable";
const DETECTOR: &str = "fingerprint";
const ENFORCEMENT_SHADOW: &str = "shadow";
const ENFORCEMENT_ENFORCE: &str = "enforce";

/// 降智检测周期任务；一次 `run_cycle` 至多执行一轮全量探测。
pub struct IntelligenceDetectionTask {
    detection: Arc<dyn DetectionStore>,
    accounts: Arc<dyn AccountStore>,
    providers: ProviderAdminRegistry,
    probe: Arc<dyn AccountProbe>,
    snapshot: Arc<dyn SnapshotControl>,
    round_in_progress: AtomicBool,
}

struct RoundGuard<'a> {
    in_progress: &'a AtomicBool,
}

struct ProbeSample {
    payload: Value,
    scores: Option<Vec<f64>>,
}

#[derive(Clone, Copy)]
struct ProbeContext<'a> {
    upstream_model: &'a UpstreamModelId,
    claimed_model: &'a str,
    scorer: &'a FingerprintScorer,
    enforcement: &'a str,
    round_id: Uuid,
}

enum DetectionProbeFailure {
    Build(ProviderAdminError),
    Request(AccountProbeError),
}

impl DetectionProbeFailure {
    fn safe_message(&self) -> String {
        match self {
            Self::Build(error) => format!("构造探测请求失败：{error}"),
            Self::Request(error) => error.client_message().to_owned(),
        }
    }
}

impl Drop for RoundGuard<'_> {
    fn drop(&mut self) {
        self.in_progress.store(false, Ordering::Release);
    }
}

impl IntelligenceDetectionTask {
    /// 组合检测存储、账号存储、Provider 注册表、探测端口与快照发布器。
    #[must_use]
    pub fn new(
        detection: Arc<dyn DetectionStore>,
        accounts: Arc<dyn AccountStore>,
        providers: ProviderAdminRegistry,
        probe: Arc<dyn AccountProbe>,
        snapshot: Arc<dyn SnapshotControl>,
    ) -> Self {
        Self {
            detection,
            accounts,
            providers,
            probe,
            snapshot,
            round_in_progress: AtomicBool::new(false),
        }
    }

    async fn cycle(&self, cancellation: &CancellationToken) -> Result<(), WorkerTaskError> {
        let Some(config) = self
            .detection
            .load_detection_config()
            .await
            .map_err(store_error)?
        else {
            return Ok(());
        };
        if !config.enabled {
            return Ok(());
        }
        let Ok(upstream_model) = UpstreamModelId::new(config.model.trim().to_owned()) else {
            warn!("指纹检测已启用但检测模型为空或不合法，跳过本轮");
            return Ok(());
        };
        let scorer = match FingerprintScorer::from_embedded_bank() {
            Ok(scorer) => Arc::new(scorer),
            Err(error) => {
                // 指纹库是判定依据；损坏时空转比误报正常更安全。
                warn!(error = %error, "ModelTrace 指纹库加载失败，跳过检测轮次");
                return Ok(());
            }
        };
        let Some(_round_guard) = self.round_due(&config).await? else {
            return Ok(());
        };
        let targets = self
            .detection
            .list_detection_targets(&config.account_scope)
            .await
            .map_err(store_error)?;
        if targets.is_empty() {
            return Ok(());
        }
        let round_id = Uuid::new_v4();
        let claimed_model = config.model.trim().to_owned();
        let enforcement = normalized_enforcement(&config.enforcement);
        info!(
            %round_id,
            targets = targets.len(),
            model = %claimed_model,
            enforcement,
            "指纹检测轮次开始"
        );
        let outcomes = stream::iter(targets.into_iter().map(|target| {
            let cancellation = cancellation.clone();
            let upstream_model = upstream_model.clone();
            let claimed_model = claimed_model.clone();
            let scorer = scorer.clone();
            async move {
                if cancellation.is_cancelled() {
                    return None;
                }
                let Ok(account_id) = ProviderAccountId::new(target.account_id.clone()) else {
                    warn!(account = target.account_id, "账号 ID 不合法，跳过探测");
                    return None;
                };
                let context = ProbeContext {
                    upstream_model: &upstream_model,
                    claimed_model: &claimed_model,
                    scorer: scorer.as_ref(),
                    enforcement,
                    round_id,
                };
                match self.probe_and_record(&account_id, &target, context).await {
                    Ok(Some((verdict, revision))) => Some((verdict, revision)),
                    Ok(None) => None,
                    Err(error) => {
                        warn!(
                            account = target.account_id,
                            %round_id,
                            error = error.as_safe_str(),
                            "写入探测记录失败，跳过该账号"
                        );
                        None
                    }
                }
            }
        }))
        .buffer_unordered(MAX_CONCURRENT_PROBES)
        .collect::<Vec<_>>()
        .await;
        let mut last_committed: Option<Revision> = None;
        let mut degraded_count = 0_usize;
        for outcome in outcomes.into_iter().flatten() {
            if outcome.0 == VERDICT_DEGRADED {
                degraded_count += 1;
            }
            if let Some(revision) = outcome.1 {
                last_committed =
                    Some(last_committed.map_or(revision, |committed| committed.max(revision)));
            }
        }
        if let Some(revision) = last_committed {
            self.publish_revision(revision).await;
        }
        info!(%round_id, degraded = degraded_count, "指纹检测轮次结束");
        Ok(())
    }

    /// 尝试开始一轮：间隔以数据库最近落库记录为准，避免实例重启后立即放行。
    async fn round_due(
        &self,
        config: &DetectionConfig,
    ) -> Result<Option<RoundGuard<'_>>, WorkerTaskError> {
        if self.round_in_progress.swap(true, Ordering::Acquire) {
            return Ok(None);
        }
        let interval = Duration::from_secs(u64::from(config.interval_secs.max(1)));
        let latest_checked_at = match self.detection.latest_detection_checked_at().await {
            Ok(checked_at) => checked_at,
            Err(error) => {
                self.round_in_progress.store(false, Ordering::Release);
                return Err(store_error(error));
            }
        };
        if latest_checked_at.is_some_and(|checked_at| is_within_interval(checked_at, interval)) {
            self.round_in_progress.store(false, Ordering::Release);
            return Ok(None);
        }
        Ok(Some(RoundGuard {
            in_progress: &self.round_in_progress,
        }))
    }

    async fn probe_and_record(
        &self,
        account_id: &ProviderAccountId,
        target: &DetectionTarget,
        context: ProbeContext<'_>,
    ) -> Result<Option<(String, Option<Revision>)>, WorkerTaskError> {
        let Ok(provider) = self.providers.require(&target.provider_kind) else {
            warn!(
                account = target.account_id,
                provider = target.provider_kind.as_str(),
                "Provider 未注册，跳过探测"
            );
            return Ok(None);
        };

        if !context
            .scorer
            .model_order()
            .iter()
            .any(|model| model == context.claimed_model)
        {
            warn!(
                model = context.claimed_model,
                "检测模型不在指纹库内，记为 unverifiable"
            );
            let payload = CHALLENGES
                .iter()
                .map(|challenge| {
                    json!({
                        "challenge_id": challenge.id,
                        "ok": false,
                        "parsed_count": 0,
                        "raw": Value::Null,
                        "note": "claimed model is not present in fingerprint bank",
                    })
                })
                .collect::<Vec<_>>();
            self.detection
                .insert_detection_record(NewDetectionRecord {
                    detection_round_id: context.round_id,
                    account_id: target.account_id.clone(),
                    degraded: false,
                    reasoning_content: None,
                    prompt_used: None,
                    suspension_released: false,
                    verdict: VERDICT_UNVERIFIABLE.to_owned(),
                    detector: DETECTOR.to_owned(),
                    margin: None,
                    claimed_model: Some(context.claimed_model.to_owned()),
                    bank_id: Some(BANK_ID.to_owned()),
                    probe_payload: Some(Value::Array(payload)),
                })
                .await
                .map_err(store_error)?;
            return Ok(Some((VERDICT_UNVERIFIABLE.to_owned(), None)));
        }

        let samples = stream::iter(CHALLENGES.iter().copied().map(|challenge| {
            let provider = provider.clone();
            async move {
                self.execute_probe(provider.as_ref(), account_id, target, &context, challenge)
                    .await
            }
        }))
        .buffer_unordered(CHALLENGES.len())
        .collect::<Vec<_>>()
        .await;
        let mut payload = Vec::with_capacity(samples.len());
        let mut score_vectors = Vec::new();
        for sample in samples {
            payload.push(sample.payload);
            if let Some(scores) = sample.scores {
                score_vectors.push(scores);
            }
        }
        let valid_count = score_vectors.len();
        let (verdict, margin) = if valid_count == CHALLENGES.len() {
            let (_, margin) =
                aggregate_scores(&score_vectors, context.scorer, context.claimed_model)
                    .map_err(|error| WorkerTaskError::safe(error.to_string()))?;
            (verdict_for_margin(margin).to_owned(), Some(margin))
        } else if valid_count > 0 {
            let (_, margin) =
                aggregate_scores(&score_vectors, context.scorer, context.claimed_model)
                    .map_err(|error| WorkerTaskError::safe(error.to_string()))?;
            (VERDICT_UNVERIFIABLE.to_owned(), Some(margin))
        } else {
            (VERDICT_UNVERIFIABLE.to_owned(), None)
        };
        let previous = if verdict == VERDICT_DEGRADED && context.enforcement == ENFORCEMENT_ENFORCE
        {
            self.detection
                .latest_actionable_detection_verdict(&target.account_id)
                .await
                .map_err(store_error)?
        } else {
            None
        };
        self.detection
            .insert_detection_record(NewDetectionRecord {
                detection_round_id: context.round_id,
                account_id: target.account_id.clone(),
                degraded: verdict == VERDICT_DEGRADED,
                reasoning_content: None,
                prompt_used: None,
                suspension_released: false,
                verdict: verdict.clone(),
                detector: DETECTOR.to_owned(),
                margin,
                claimed_model: Some(context.claimed_model.to_owned()),
                bank_id: Some(BANK_ID.to_owned()),
                probe_payload: Some(Value::Array(payload)),
            })
            .await
            .map_err(store_error)?;
        let revision = self
            .reconcile_suspension(
                account_id,
                target,
                &verdict,
                previous.as_deref(),
                context.enforcement,
                context.round_id,
            )
            .await?;
        Ok(Some((verdict, revision)))
    }

    async fn execute_probe(
        &self,
        provider: &dyn ProviderAdmin,
        account_id: &ProviderAccountId,
        target: &DetectionTarget,
        context: &ProbeContext<'_>,
        challenge: FingerprintChallenge,
    ) -> ProbeSample {
        let operation = match provider
            .intelligence_detection_operation(context.upstream_model, challenge.prompt)
        {
            Ok(operation) => operation,
            Err(error) => {
                return ProbeSample {
                    payload: failure_payload(
                        challenge,
                        DetectionProbeFailure::Build(error).safe_message(),
                    ),
                    scores: None,
                };
            }
        };
        let result = self
            .probe
            .probe(AccountProbeRequest {
                account_id: account_id.clone(),
                provider_kind: target.provider_kind.clone(),
                upstream_model: context.upstream_model.clone(),
                operation,
            })
            .await;
        match result {
            Ok(AccountProbeResult { text, .. }) => {
                let raw = text.concat();
                match context.scorer.score(&raw, context.claimed_model) {
                    Ok(scored) if scored.parsed_numbers > 0 => ProbeSample {
                        payload: json!({
                            "challenge_id": challenge.id,
                            "ok": true,
                            "parsed_count": scored.parsed_numbers,
                            "raw": truncate_raw(&raw),
                            "note": Value::Null,
                        }),
                        scores: Some(scored.scores),
                    },
                    Ok(scored) => ProbeSample {
                        payload: json!({
                            "challenge_id": challenge.id,
                            "ok": false,
                            "parsed_count": scored.parsed_numbers,
                            "raw": truncate_raw(&raw),
                            "note": "response contained no valid fingerprint numbers",
                        }),
                        scores: None,
                    },
                    Err(error) => ProbeSample {
                        payload: failure_payload(challenge, error.to_string()),
                        scores: None,
                    },
                }
            }
            Err(error) => ProbeSample {
                payload: failure_payload(
                    challenge,
                    DetectionProbeFailure::Request(error).safe_message(),
                ),
                scores: None,
            },
        }
    }

    /// 按当前结论翻转调度暂停；恢复只针对检测来源，人工暂停不受影响。
    async fn reconcile_suspension(
        &self,
        account_id: &ProviderAccountId,
        target: &DetectionTarget,
        verdict: &str,
        previous_verdict: Option<&str>,
        enforcement: &str,
        round_id: Uuid,
    ) -> Result<Option<Revision>, WorkerTaskError> {
        let suspension = match verdict {
            VERDICT_NORMAL
                if target.scheduling_suspended_by
                    == Some(SchedulingSuspensionSource::Detection) =>
            {
                Some(None)
            }
            VERDICT_DEGRADED
                if should_suspend_degraded(
                    enforcement,
                    previous_verdict,
                    target.scheduling_suspended,
                ) =>
            {
                Some(Some(SchedulingSuspensionSource::Detection))
            }
            _ => None,
        };
        let Some(suspension) = suspension else {
            return Ok(None);
        };
        let context = MutationContext {
            actor: MutationActor::System,
            request_id: format!("intelligence-detection-{round_id}"),
        };
        let result = self
            .accounts
            .set_scheduling_suspended(account_id, suspension, &context)
            .await
            .map_err(store_error)?;
        if suspension.is_none()
            && let Err(error) = self
                .detection
                .mark_suspension_released(round_id, &target.account_id)
                .await
        {
            warn!(account = target.account_id, %round_id, error = %error, "标记检测恢复失败");
        }
        if let Ok(provider) = self.providers.require(&target.provider_kind) {
            provider
                .account_facts_changed(std::slice::from_ref(account_id))
                .await;
        }
        info!(
            account = target.account_id,
            suspended = suspension.is_some(),
            %round_id,
            "检测结果翻转了账号调度暂停"
        );
        Ok(Some(result.config_revision))
    }

    async fn publish_revision(&self, revision: Revision) {
        match ConfigRevision::new(revision.get()) {
            Ok(revision) => self.snapshot.publish_committed(revision).await,
            Err(_) => warn!("已提交的配置版本不合法，等待 runtime 周期对账收敛"),
        }
    }
}

impl ScheduledTask for IntelligenceDetectionTask {
    fn run_cycle(&self, context: WorkerCycleContext) -> BoxFuture<'_, Result<(), WorkerTaskError>> {
        let cancellation = context.cancellation().clone();
        Box::pin(async move { self.cycle(&cancellation).await })
    }
}

fn store_error(error: AdminStoreError) -> WorkerTaskError {
    WorkerTaskError::safe(error.to_string())
}

fn is_within_interval(checked_at: DateTime<Utc>, interval: Duration) -> bool {
    let elapsed = Utc::now().signed_duration_since(checked_at);
    elapsed < chrono::Duration::zero()
        || chrono::Duration::from_std(interval).is_ok_and(|limit| elapsed < limit)
}

fn normalized_enforcement(value: &str) -> &'static str {
    if value.trim().eq_ignore_ascii_case(ENFORCEMENT_ENFORCE) {
        ENFORCEMENT_ENFORCE
    } else {
        ENFORCEMENT_SHADOW
    }
}

pub fn verdict_for_margin(margin: f64) -> &'static str {
    if margin >= MARGIN_THRESHOLD {
        VERDICT_NORMAL
    } else if margin <= -MARGIN_THRESHOLD {
        VERDICT_DEGRADED
    } else {
        VERDICT_UNDETERMINED
    }
}

pub fn should_suspend_degraded(
    enforcement: &str,
    previous_verdict: Option<&str>,
    already_suspended: bool,
) -> bool {
    enforcement == ENFORCEMENT_ENFORCE
        && previous_verdict == Some(VERDICT_DEGRADED)
        && !already_suspended
}

fn aggregate_scores(
    score_vectors: &[Vec<f64>],
    scorer: &FingerprintScorer,
    claimed_model: &str,
) -> Result<(String, f64), model_fingerprint::FingerprintError> {
    let width = score_vectors.first().map_or(0, Vec::len);
    let mut scores = vec![0.0; width];
    for vector in score_vectors {
        for (sum, score) in scores.iter_mut().zip(vector) {
            *sum += score;
        }
    }
    let divisor = score_vectors.len() as f64;
    for score in &mut scores {
        *score /= divisor;
    }
    let claimed_index = scorer
        .model_order()
        .iter()
        .position(|model| model == claimed_model)
        .ok_or_else(|| {
            model_fingerprint::FingerprintError::UnknownModel(claimed_model.to_owned())
        })?;
    let mut rank_one = 0;
    let mut max_other = f64::NEG_INFINITY;
    for (index, score) in scores.iter().copied().enumerate() {
        if score > scores[rank_one] {
            rank_one = index;
        }
        if index != claimed_index {
            max_other = max_other.max(score);
        }
    }
    Ok((
        scorer.model_order()[rank_one].clone(),
        scores[claimed_index] - max_other,
    ))
}

fn failure_payload(challenge: FingerprintChallenge, note: String) -> Value {
    json!({
        "challenge_id": challenge.id,
        "ok": false,
        "parsed_count": 0,
        "raw": Value::Null,
        "note": truncate_raw(&note),
    })
}

fn truncate_raw(value: &str) -> String {
    value.chars().take(8192).collect()
}
