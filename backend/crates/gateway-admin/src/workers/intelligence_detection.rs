//! 降智检测 Worker：周期向账号发送固定探测提示词，按降智指征翻转调度暂停。
//!
//! Host 以固定短间隔调用 `run_cycle`；每个周期先读最新检测配置，未启用或距离上一
//! 轮不足 `interval_secs` 时直接空转，因此配置修改无需重启即可生效。探测通过
//! [`AccountProbe`] 走真实执行链（该链路对诊断请求跳过本地调度资格投影，暂停中的
//! 账号也能被探测到），存储读写全部经由 Admin ports，不触碰 HTTP 层与 Provider SDK。
//!
//! 自动恢复带来源守卫：只有 `scheduling_suspended_by = detection` 的账号会在探测
//! 恢复正常后被解除暂停，人工暂停不受检测结果影响。当前部署为单副本，不需要
//! Redis leader lease。

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
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

/// 探测固定提示词；降智账号对它的答复会退化为纯文本描述。
const DETECTION_PROMPT: &str = "创建一个 HTML，内容是 SVG 绘制一个鹈鹕骑自行车的 2D 动画。";

/// 降智指征关键词；ASCII 大小写不敏感，命中任意一个即判定降智。
const DEGRADED_PHRASES: &[&str] = &["内嵌 SVG 和 CSS", "inline SVG", "SVG and CSS"];

/// 降智检测周期任务；一次 `run_cycle` 至多执行一轮全量探测。
pub struct IntelligenceDetectionTask {
    detection: Arc<dyn DetectionStore>,
    accounts: Arc<dyn AccountStore>,
    providers: ProviderAdminRegistry,
    probe: Arc<dyn AccountProbe>,
    snapshot: Arc<dyn SnapshotControl>,
    last_round_started_at: Mutex<Option<Instant>>,
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
            last_round_started_at: Mutex::new(None),
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
        if !self.round_due(&config) {
            return Ok(());
        }
        let targets = self
            .detection
            .list_detection_targets(&config.account_scope)
            .await
            .map_err(store_error)?;
        if targets.is_empty() {
            return Ok(());
        }
        let round_id = Uuid::new_v4();
        info!(
            %round_id,
            targets = targets.len(),
            model = upstream_model.as_str(),
            "降智检测轮次开始"
        );
        let mut last_committed = None;
        let mut degraded_count = 0_usize;
        for target in &targets {
            if cancellation.is_cancelled() {
                break;
            }
            let Ok(account_id) = ProviderAccountId::new(target.account_id.clone()) else {
                warn!(account = target.account_id, "账号 ID 不合法，跳过探测");
                continue;
            };
            let Some(degraded) = self
                .probe_and_record(&account_id, target, &upstream_model, round_id)
                .await?
            else {
                continue;
            };
            degraded_count += usize::from(degraded);
            if let Some(revision) = self
                .reconcile_suspension(&account_id, target, degraded, round_id)
                .await?
            {
                last_committed = Some(revision);
            }
        }
        if let Some(revision) = last_committed {
            self.publish_revision(revision).await;
        }
        info!(%round_id, degraded = degraded_count, "降智检测轮次结束");
        Ok(())
    }

    /// 距上一轮开始不足 `interval_secs` 时返回 `false`；到期则记录本轮开始时间。
    fn round_due(&self, config: &DetectionConfig) -> bool {
        let interval = Duration::from_secs(u64::from(config.interval_secs.max(1)));
        let mut guard = self
            .last_round_started_at
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match *guard {
            Some(started_at) if started_at.elapsed() < interval => false,
            _ => {
                *guard = Some(Instant::now());
                true
            }
        }
    }

    /// 探测单个账号并落检测记录；探测失败只告警不落记录，返回 `None`。
    async fn probe_and_record(
        &self,
        account_id: &ProviderAccountId,
        target: &DetectionTarget,
        upstream_model: &UpstreamModelId,
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
        let operation = match provider.connection_test_operation(upstream_model, DETECTION_PROMPT) {
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
        let text = match result {
            Ok(result) => result.text.concat(),
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
        let matched = matched_phrases(&text);
        let degraded = !matched.is_empty();
        self.detection
            .insert_detection_record(NewDetectionRecord {
                detection_round_id: round_id,
                account_id: target.account_id.clone(),
                degraded,
                html_content: Some(extract_html_document(&text)),
                matched_phrases: matched,
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

/// 返回响应文本命中的降智指征；ASCII 大小写不敏感。
fn matched_phrases(text: &str) -> Vec<String> {
    let haystack = text.to_ascii_lowercase();
    DEGRADED_PHRASES
        .iter()
        .filter(|phrase| haystack.contains(&phrase.to_ascii_lowercase()))
        .map(|phrase| (*phrase).to_owned())
        .collect()
}

/// 从响应文本中提取 HTML 文档供检测记录回放；找不到 HTML 结构时保留全文。
fn extract_html_document(text: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::{extract_html_document, matched_phrases};

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
}
