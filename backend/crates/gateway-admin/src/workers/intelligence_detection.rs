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
    engine::probe::{AccountProbe, AccountProbeRequest},
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
        provider::ProviderAdminRegistry,
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
        let round_id = Uuid::new_v4();
        let prompt = prompt_for_round(round_id);
        info!(
            %round_id,
            targets = targets.len(),
            model = upstream_model.as_str(),
            "降智检测轮次开始"
        );
        let outcomes = stream::iter(targets.into_iter().map(|target| {
            let cancellation = cancellation.clone();
            let upstream_model = upstream_model.clone();
            let prompt = prompt.clone();
            async move {
                if cancellation.is_cancelled() {
                    return None;
                }
                let Ok(account_id) = ProviderAccountId::new(target.account_id.clone()) else {
                    warn!(account = target.account_id, "账号 ID 不合法，跳过探测");
                    return None;
                };
                let degraded = match self
                    .probe_and_record(&account_id, &target, &upstream_model, &prompt, round_id)
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
        let operation = match provider.intelligence_detection_operation(upstream_model, prompt) {
            Ok(operation) => operation,
            Err(error) => {
                warn!(
                    account = target.account_id,
                    %error,
                    "构造探测 operation 失败，跳过该账号"
                );
                return Ok(None);
            }
        };
        let result = self
            .probe
            .probe(AccountProbeRequest {
                account_id: account_id.clone(),
                provider_kind: target.provider_kind.clone(),
                upstream_model: upstream_model.clone(),
                operation,
            })
            .await;
        let (text, reasoning) = match result {
            Ok(result) => (result.text.concat(), result.reasoning.concat()),
            Err(error) => {
                warn!(
                    account = target.account_id,
                    %round_id,
                    error = error.client_message(),
                    "探测请求失败，本轮不记录该账号也不改变其调度状态"
                );
                return Ok(None);
            }
        };
        let matched = matched_phrases(&format!("{reasoning}{text}"));
        let degraded = !matched.is_empty();
        self.detection
            .insert_detection_record(NewDetectionRecord {
                detection_round_id: round_id,
                account_id: target.account_id.clone(),
                degraded,
                html_content: Some(extract_html_document(&text)),
                reasoning_content: Some(reasoning),
                prompt_used: Some(prompt.to_owned()),
                matched_phrases: matched,
                suspension_released: false,
            })
            .await
            .map_err(store_error)?;
        Ok(Some(degraded))
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
    let normalized: String = text
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    let direct = [
        "内嵌svg",
        "内联svg",
        "内嵌的svg",
        "内联的svg",
        "inlinesvg",
        "svgandcss",
    ];
    let mut matched = direct
        .iter()
        .filter(|phrase| normalized.contains(**phrase))
        .map(|phrase| (*phrase).to_owned())
        .collect::<Vec<_>>();
    for marker in ["内嵌", "内联"] {
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
