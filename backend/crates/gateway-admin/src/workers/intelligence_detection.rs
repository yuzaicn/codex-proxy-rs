//! 降智检测 Worker：周期向账号发送固定探测提示词，按降智指征翻转调度暂停。
//!
//! Host 以固定短间隔调用 `run_cycle`；每个周期先读最新检测配置，未启用或距离上一
//! 轮不足 `interval_secs` 时直接空转，因此配置修改无需重启即可生效。探测通过
//! [`AccountProbe`] 走真实执行链（该链路对诊断请求跳过本地调度资格投影，暂停中的
//! 账号也能被探测到），存储读写全部经由 Admin ports，不触碰 HTTP 层与 Provider SDK。
//!
//! 自动恢复带来源守卫：只有 `scheduling_suspended_by = detection` 的账号会在探测
//! 恢复正常后被解除暂停，人工暂停不受检测结果影响。多实例部署通过 Host 提供的
//! Redis leader lease 保证同一轮只有一个实例执行。

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
    error::GatewayErrorKind,
    lifecycle::CancellationToken,
    routing::{ConfigRevision, UpstreamModelId},
    runtime::SnapshotControl,
    task::{ScheduledTask, WorkerCycleContext, WorkerTaskError},
};
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

/// 探测提示词模板；每轮从动物清单中稳定选择一个动物。
const DETECTION_PROMPT_TEMPLATE: &str = "创建一个HTML，内容是SVG绘制一个{animal}骑自行车的2D动画。";
const DETECTION_ANIMALS: &[&str] = &[
    "企鹅",
    "鹈鹕",
    "猫",
    "狗",
    "熊猫",
    "长颈鹿",
    "章鱼",
    "袋鼠",
    "犀牛",
    "火烈鸟",
    "树懒",
    "浣熊",
];

/// 单轮探测允许同时在途的账号数。
const MAX_CONCURRENT_PROBES: usize = 16;

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

enum DetectionProbeFailure {
    Build(ProviderAdminError),
    Request(AccountProbeError),
}

impl DetectionProbeFailure {
    fn is_invalid_request(&self) -> bool {
        matches!(
            self,
            Self::Request(error) if error.kind() == GatewayErrorKind::InvalidRequest
        )
    }

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
            warn!("降智检测已启用但检测模型为空或不合法，跳过本轮");
            return Ok(());
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
        let advertised_efforts = self
            .snapshot
            .supported_reasoning_efforts(&targets[0].provider_kind, &upstream_model);
        let reasoning_effort =
            resolve_reasoning_effort(&config.reasoning_effort, &advertised_efforts);
        let round_id = Uuid::new_v4();
        let prompt = prompt_for_round(round_id);
        info!(
            %round_id,
            targets = targets.len(),
            model = upstream_model.as_str(),
            reasoning_effort,
            "降智检测轮次开始"
        );
        let outcomes = stream::iter(targets.into_iter().map(|target| {
            let cancellation = cancellation.clone();
            let upstream_model = upstream_model.clone();
            let prompt = prompt.clone();
            let reasoning_effort = reasoning_effort.clone();
            async move {
                if cancellation.is_cancelled() {
                    return None;
                }
                let Ok(account_id) = ProviderAccountId::new(target.account_id.clone()) else {
                    warn!(account = target.account_id, "账号 ID 不合法，跳过探测");
                    return None;
                };
                let degraded = match self
                    .probe_and_record(
                        &account_id,
                        &target,
                        &upstream_model,
                        &prompt,
                        &reasoning_effort,
                        round_id,
                    )
                    .await
                {
                    Ok(Some(degraded)) => degraded,
                    Ok(None) => return None,
                    Err(error) => {
                        warn!(
                            account = target.account_id,
                            %round_id,
                            error = error.as_safe_str(),
                            "写入探测记录失败，跳过该账号"
                        );
                        return None;
                    }
                };
                let revision = match self
                    .reconcile_suspension(&account_id, &target, degraded, round_id)
                    .await
                {
                    Ok(revision) => revision,
                    Err(error) => {
                        warn!(
                            account = target.account_id,
                            %round_id,
                            error = error.as_safe_str(),
                            "更新账号调度状态失败，保留该账号本轮记录"
                        );
                        None
                    }
                };
                Some((degraded, revision))
            }
        }))
        .buffer_unordered(MAX_CONCURRENT_PROBES)
        .collect::<Vec<_>>()
        .await;
        let mut last_committed: Option<Revision> = None;
        let mut degraded_count = 0_usize;
        for outcome in outcomes {
            let Some((degraded, revision)) = outcome else {
                continue;
            };
            degraded_count += usize::from(degraded);
            if let Some(revision) = revision {
                last_committed =
                    Some(last_committed.map_or(revision, |committed| committed.max(revision)));
            }
        }
        if let Some(revision) = last_committed {
            self.publish_revision(revision).await;
        }
        info!(%round_id, degraded = degraded_count, "降智检测轮次结束");
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

    /// 探测单个账号并落检测记录；探测失败只告警不落记录，返回 `None`。
    async fn probe_and_record(
        &self,
        account_id: &ProviderAccountId,
        target: &DetectionTarget,
        upstream_model: &UpstreamModelId,
        prompt: &str,
        reasoning_effort: &str,
        round_id: Uuid,
    ) -> Result<Option<bool>, WorkerTaskError> {
        let Ok(provider) = self.providers.require(&target.provider_kind) else {
            warn!(
                account = target.account_id,
                provider = target.provider_kind.as_str(),
                "Provider 未注册，跳过探测"
            );
            return Ok(None);
        };
        let mut effective_effort = reasoning_effort.to_owned();
        let mut fallback_note = None;
        let mut result = self
            .execute_probe(
                provider.as_ref(),
                account_id,
                target,
                upstream_model,
                prompt,
                &effective_effort,
            )
            .await;
        if effective_effort == "xhigh"
            && result
                .as_ref()
                .is_err_and(DetectionProbeFailure::is_invalid_request)
        {
            effective_effort = "high".to_owned();
            fallback_note =
                Some("reasoning_effort=xhigh 被上游拒绝，本轮已自动退档到 high".to_owned());
            result = self
                .execute_probe(
                    provider.as_ref(),
                    account_id,
                    target,
                    upstream_model,
                    prompt,
                    &effective_effort,
                )
                .await;
        }
        let (text, reasoning) = match result {
            Ok(result) => (result.text.concat(), result.reasoning.concat()),
            Err(error) => {
                let failure = format!(
                    "{}探测失败（reasoning_effort={effective_effort}）：{}",
                    fallback_note
                        .as_deref()
                        .map_or(String::new(), |note| format!("{note}；")),
                    error.safe_message()
                );
                warn!(
                    account = target.account_id,
                    %round_id,
                    error = %failure,
                    "探测请求失败，写入失败留痕；不改变其调度状态"
                );
                self.detection
                    .insert_detection_record(NewDetectionRecord {
                        detection_round_id: round_id,
                        account_id: target.account_id.clone(),
                        degraded: false,
                        html_content: None,
                        reasoning_content: Some(failure),
                        prompt_used: Some(prompt.to_owned()),
                        matched_phrases: Vec::new(),
                        suspension_released: false,
                    })
                    .await
                    .map_err(store_error)?;
                return Ok(None);
            }
        };
        let matched = matched_phrases(&format!("{reasoning}\0{text}"));
        let degraded = !matched.is_empty();
        let reasoning_content = fallback_note
            .map(|note| format!("[{note}]\n{reasoning}"))
            .unwrap_or(reasoning);
        self.detection
            .insert_detection_record(NewDetectionRecord {
                detection_round_id: round_id,
                account_id: target.account_id.clone(),
                degraded,
                html_content: Some(extract_html_document(&text)),
                reasoning_content: Some(reasoning_content),
                prompt_used: Some(prompt.to_owned()),
                matched_phrases: matched,
                suspension_released: false,
            })
            .await
            .map_err(store_error)?;
        Ok(Some(degraded))
    }

    async fn execute_probe(
        &self,
        provider: &dyn ProviderAdmin,
        account_id: &ProviderAccountId,
        target: &DetectionTarget,
        upstream_model: &UpstreamModelId,
        prompt: &str,
        reasoning_effort: &str,
    ) -> Result<AccountProbeResult, DetectionProbeFailure> {
        let operation = provider
            .intelligence_detection_operation(upstream_model, prompt, reasoning_effort)
            .map_err(DetectionProbeFailure::Build)?;
        self.probe
            .probe(AccountProbeRequest {
                account_id: account_id.clone(),
                provider_kind: target.provider_kind.clone(),
                upstream_model: upstream_model.clone(),
                operation,
            })
            .await
            .map_err(DetectionProbeFailure::Request)
    }

    /// 按检测结论翻转调度暂停；恢复只针对检测来源的暂停，人工暂停不受影响。
    async fn reconcile_suspension(
        &self,
        account_id: &ProviderAccountId,
        target: &DetectionTarget,
        degraded: bool,
        round_id: Uuid,
    ) -> Result<Option<Revision>, WorkerTaskError> {
        let suspension = match (
            degraded,
            target.scheduling_suspended,
            target.scheduling_suspended_by,
        ) {
            (true, false, _) => Some(SchedulingSuspensionSource::Detection),
            (false, true, Some(SchedulingSuspensionSource::Detection)) => None,
            _ => return Ok(None),
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

    /// 发布已提交的 config revision，让后续请求感知调度状态变化。
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

/// 解析检测推理档位。显式配置原样优先；`auto` 从 catalog 声明中按稳定阶梯取顶，
/// catalog 缺失或不含已知档位时退化到 `xhigh`。
#[must_use]
pub fn resolve_reasoning_effort(configured: &str, supported: &[String]) -> String {
    let configured = configured.trim().to_ascii_lowercase();
    if configured != "auto" {
        return configured;
    }
    supported
        .iter()
        .filter_map(|effort| {
            let effort = effort.trim().to_ascii_lowercase();
            reasoning_effort_rank(&effort).map(|rank| (rank, effort))
        })
        .max_by_key(|(rank, _)| *rank)
        .map_or_else(|| "xhigh".to_owned(), |(_, effort)| effort)
}

fn reasoning_effort_rank(effort: &str) -> Option<u8> {
    match effort {
        "none" => Some(0),
        "minimal" => Some(1),
        "low" => Some(2),
        "medium" => Some(3),
        "high" => Some(4),
        "xhigh" => Some(5),
        "max" => Some(6),
        _ => None,
    }
}

/// 为一轮检测生成可复现的提示词；同一轮所有账号共享这一提示词。
#[must_use]
pub fn prompt_for_round(round_id: Uuid) -> String {
    let animal = DETECTION_ANIMALS[(round_id.as_bytes()[0] as usize) % DETECTION_ANIMALS.len()];
    DETECTION_PROMPT_TEMPLATE.replace("{animal}", animal)
}

/// 返回响应（思考过程在前、最终答复在后）命中的降智指征。
///
/// 匹配前会移除所有 Unicode 空白并转为 ASCII 小写，以覆盖模型常见的中文
/// 排版变体。除固定词根外，还接受“内嵌/内联”与 `svg` 相距不超过 10 个字符的
/// 写法；返回值使用规范化标签，便于审计记录说明命中原因。
#[must_use]
pub fn matched_phrases(text: &str) -> Vec<String> {
    let mut matched = Vec::new();
    for segment in text.split('\0') {
        for phrase in matched_phrases_in_segment(segment) {
            if !matched.contains(&phrase) {
                matched.push(phrase);
            }
        }
    }
    matched
}

fn matched_phrases_in_segment(text: &str) -> Vec<String> {
    let normalized = text
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    let direct = ["内嵌svg", "内联svg", "内嵌的svg", "内联的svg", "inlinesvg"];
    let mut matched = direct
        .iter()
        .filter(|phrase| normalized.contains(**phrase))
        .map(|phrase| (*phrase).to_owned())
        .collect::<Vec<_>>();
    for marker in ["内嵌", "内联", "inline", "embed"] {
        if !matched.iter().any(|phrase| phrase.starts_with(marker))
            && has_nearby_svg(&normalized, marker)
        {
            matched.push(format!("{marker}与svg间隔≤10字符"));
        }
    }
    matched
}

fn has_nearby_svg(text: &str, marker: &str) -> bool {
    let marker_positions = text.match_indices(marker).map(|(index, _)| index);
    marker_positions.into_iter().any(|marker_start| {
        let marker_end = marker_start + marker.len();
        let after_marker = text
            .get(marker_end..)
            .and_then(|suffix| suffix.find("svg").map(|index| marker_end + index));
        let before_marker = text
            .get(..marker_start)
            .and_then(|prefix| prefix.rfind("svg"));
        after_marker.is_some_and(|svg_start| text[marker_end..svg_start].chars().count() <= 10)
            || before_marker.is_some_and(|svg_start| {
                text[svg_start + "svg".len()..marker_start].chars().count() <= 10
            })
    })
}

/// 从响应文本中提取 HTML 文档供检测记录回放；找不到 HTML 结构时保留全文。
#[must_use]
pub fn extract_html_document(text: &str) -> String {
    if let Some(fenced) = extract_fenced_html(text) {
        return fenced;
    }
    let haystack = text.to_ascii_lowercase();
    let start = haystack
        .find("<!doctype")
        .or_else(|| haystack.find("<html"));
    if let Some(start) = start {
        let document = &text[start..];
        let end = document
            .to_ascii_lowercase()
            .find("</html>")
            .map_or(document.len(), |index| index + "</html>".len());
        return document[..end].to_owned();
    }
    text.to_owned()
}

/// 提取第一个 ```html 围栏代码块的内容。
fn extract_fenced_html(text: &str) -> Option<String> {
    let start = text.find("```html")? + "```html".len();
    let body = text.get(start..)?;
    let body = body.strip_prefix('\n').unwrap_or(body);
    let end = body.find("```").unwrap_or(body.len());
    let content = body[..end].trim();
    (!content.is_empty()).then(|| content.to_owned())
}
