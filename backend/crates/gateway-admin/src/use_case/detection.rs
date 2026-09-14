//! 降智检测配置与检测记录用例。

use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use gateway_core::{account::ProviderAccountId, runtime::SnapshotControl};

use crate::{
    model::{
        AdminError, MutationContext,
        detection::{
            DetectionAccountScope, DetectionConfig, DetectionRecord, DetectionRecordQuery,
            DetectionRound, ReplaceDetectionConfig,
        },
    },
    ports::{provider::ProviderAdminRegistry, store::DetectionStore},
};

use super::{map_store_error, publish_committed};

const MIN_INTERVAL_SECS: u32 = 60;
const MAX_INTERVAL_SECS: u32 = 86_400;
const MAX_MODEL_BYTES: usize = 256;
const MAX_REASONING_EFFORT_BYTES: usize = 32;
const MAX_SELECTED_ACCOUNTS: usize = 1000;
const MAX_RECORD_PAGE_SIZE: u32 = 500;
const ROUNDS_LIMIT: u32 = 200;

/// 系统设置检测区块与检测记录页消费的服务。
#[async_trait]
pub trait DetectionService: Send + Sync {
    async fn config(&self) -> Result<DetectionConfig, AdminError>;

    async fn replace_config(
        &self,
        context: &MutationContext,
        command: ReplaceDetectionConfig,
    ) -> Result<DetectionConfig, AdminError>;

    async fn records(
        &self,
        query: DetectionRecordQuery,
    ) -> Result<Vec<DetectionRecord>, AdminError>;

    async fn rounds(&self) -> Result<Vec<DetectionRound>, AdminError>;

    /// 读取单条检测记录的 HTML 原文；记录不存在或没有 HTML 内容时返回 NotFound。
    async fn record_html(&self, record_id: i64) -> Result<String, AdminError>;
}

pub(crate) struct DefaultDetectionService {
    store: Arc<dyn DetectionStore>,
    snapshot: Arc<dyn SnapshotControl>,
    providers: ProviderAdminRegistry,
}

impl DefaultDetectionService {
    #[must_use]
    pub(crate) fn new(
        store: Arc<dyn DetectionStore>,
        snapshot: Arc<dyn SnapshotControl>,
        providers: ProviderAdminRegistry,
    ) -> Self {
        Self {
            store,
            snapshot,
            providers,
        }
    }
}

#[async_trait]
impl DetectionService for DefaultDetectionService {
    async fn config(&self) -> Result<DetectionConfig, AdminError> {
        let config = self
            .store
            .load_detection_config()
            .await
            .map_err(|error| map_store_error(error, "detection config"))?;
        Ok(config.unwrap_or_else(|| DetectionConfig::initial(Utc::now())))
    }

    async fn replace_config(
        &self,
        context: &MutationContext,
        command: ReplaceDetectionConfig,
    ) -> Result<DetectionConfig, AdminError> {
        let command = normalize_config(command)?;
        let mutation = self
            .store
            .replace_detection_config(command, context)
            .await
            .map_err(|error| map_store_error(error, "detection config"))?;
        publish_committed(self.snapshot.as_ref(), mutation.config_revision).await?;
        Ok(mutation.config)
    }

    async fn records(
        &self,
        query: DetectionRecordQuery,
    ) -> Result<Vec<DetectionRecord>, AdminError> {
        if query.page == 0 || query.page_size == 0 || query.page_size > MAX_RECORD_PAGE_SIZE {
            return Err(AdminError::invalid("检测记录分页参数不合法"));
        }
        let mut records = self
            .store
            .list_detection_records(query)
            .await
            .map_err(|error| map_store_error(error, "detection records"))?;
        for record in &mut records {
            record.account_plan_type_display = self.providers.resolve_account_plan(
                &record.account_provider_kind,
                &mut record.account_plan_type,
                None,
            );
        }
        Ok(records)
    }

    async fn rounds(&self) -> Result<Vec<DetectionRound>, AdminError> {
        self.store
            .list_detection_rounds(ROUNDS_LIMIT)
            .await
            .map_err(|error| map_store_error(error, "detection rounds"))
    }

    async fn record_html(&self, record_id: i64) -> Result<String, AdminError> {
        if record_id <= 0 {
            return Err(AdminError::invalid("检测记录 ID 不合法"));
        }
        self.store
            .load_detection_record_html(record_id)
            .await
            .map_err(|error| map_store_error(error, "detection record html"))?
            .ok_or_else(|| AdminError::not_found("检测记录不存在或没有 HTML 内容"))
    }
}

/// 校验并归一化配置命令：模型去空白、选中账号去重。
fn normalize_config(command: ReplaceDetectionConfig) -> Result<ReplaceDetectionConfig, AdminError> {
    if !(MIN_INTERVAL_SECS..=MAX_INTERVAL_SECS).contains(&command.interval_secs) {
        return Err(AdminError::invalid("检测间隔需在 60 到 86400 秒之间"));
    }
    let model = command.model.trim().to_owned();
    if model.len() > MAX_MODEL_BYTES || model.bytes().any(|byte| byte.is_ascii_control()) {
        return Err(AdminError::invalid("检测模型不合法"));
    }
    if command.enabled && model.is_empty() {
        return Err(AdminError::invalid("启用检测前必须填写检测模型"));
    }
    let reasoning_effort = command.reasoning_effort.trim().to_ascii_lowercase();
    if reasoning_effort.is_empty()
        || reasoning_effort.len() > MAX_REASONING_EFFORT_BYTES
        || reasoning_effort.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(AdminError::invalid("检测推理档位不合法"));
    }
    let account_scope = match command.account_scope {
        DetectionAccountScope::AllAccounts => DetectionAccountScope::AllAccounts,
        DetectionAccountScope::SelectedAccounts { account_ids } => {
            if account_ids.len() > MAX_SELECTED_ACCOUNTS {
                return Err(AdminError::invalid("检测账号数量超出上限"));
            }
            let mut seen = BTreeSet::new();
            let mut deduped = Vec::with_capacity(account_ids.len());
            for account_id in account_ids {
                ProviderAccountId::new(account_id.clone())
                    .map_err(|_| AdminError::invalid("检测账号 ID 不合法"))?;
                if seen.insert(account_id.clone()) {
                    deduped.push(account_id);
                }
            }
            DetectionAccountScope::SelectedAccounts {
                account_ids: deduped,
            }
        }
    };
    Ok(ReplaceDetectionConfig {
        enabled: command.enabled,
        account_scope,
        interval_secs: command.interval_secs,
        model,
        reasoning_effort,
    })
}
