//! 降智检测配置与检测记录的语义模型。

use chrono::{DateTime, Utc};
use uuid::Uuid;

use gateway_core::{account::SchedulingSuspensionSource, routing::ProviderKind};

use super::Revision;

/// 降智检测的账号范围。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetectionAccountScope {
    /// 检测全部账号。
    AllAccounts,
    /// 只检测选中的账号。
    SelectedAccounts { account_ids: Vec<String> },
}

/// 降智检测全局配置事实（单行）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectionConfig {
    pub enabled: bool,
    pub account_scope: DetectionAccountScope,
    pub interval_secs: u32,
    pub model: String,
    pub updated_at: DateTime<Utc>,
}

impl DetectionConfig {
    /// 配置行尚未写入时的初始配置。
    #[must_use]
    pub fn initial(updated_at: DateTime<Utc>) -> Self {
        Self {
            enabled: false,
            account_scope: DetectionAccountScope::AllAccounts,
            interval_secs: 3600,
            model: String::new(),
            updated_at,
        }
    }
}

/// 原子替换降智检测配置的命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplaceDetectionConfig {
    pub enabled: bool,
    pub account_scope: DetectionAccountScope,
    pub interval_secs: u32,
    pub model: String,
}

/// 检测配置替换结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectionConfigMutation {
    pub config_revision: Revision,
    pub config: DetectionConfig,
}

/// 一条检测记录及其账号身份投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectionRecord {
    pub id: i64,
    pub detection_round_id: Uuid,
    pub account_id: String,
    pub account_email: Option<String>,
    pub account_name: Option<String>,
    pub account_provider_kind: String,
    pub account_plan_type: Option<String>,
    /// 由用例按 Provider 注册表解析的套餐展示名称；Store 层不填。
    pub account_plan_type_display: Option<String>,
    pub checked_at: DateTime<Utc>,
    pub degraded: bool,
    pub scheduling_suspended: bool,
    /// 探测响应中的思考过程，供降智判定复核。
    pub reasoning_content: Option<String>,
    /// 本轮实际发送给模型的提示词。
    pub prompt_used: Option<String>,
}

/// 检测记录分页查询；页码从 1 开始。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectionRecordQuery {
    pub detection_round_id: Option<Uuid>,
    pub page: u32,
    pub page_size: u32,
}

/// 一个检测批次的聚合视图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectionRound {
    pub detection_round_id: Uuid,
    pub checked_at: DateTime<Utc>,
    pub degraded_count: u64,
    pub normal_count: u64,
}

/// 检测 Worker 视角的一个待探测账号及其当前调度暂停事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectionTarget {
    pub account_id: String,
    pub provider_kind: ProviderKind,
    pub scheduling_suspended: bool,
    pub scheduling_suspended_by: Option<SchedulingSuspensionSource>,
}

/// 检测 Worker 写入的一条新检测记录；`checked_at` 由存储层落库时间决定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewDetectionRecord {
    pub detection_round_id: Uuid,
    pub account_id: String,
    pub degraded: bool,
    pub html_content: Option<String>,
    pub reasoning_content: Option<String>,
    pub prompt_used: Option<String>,
    pub matched_phrases: Vec<String>,
}
