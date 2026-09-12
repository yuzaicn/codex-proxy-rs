use chrono::{DateTime, Utc};
use gateway_core::account::{AccountStatus, ProviderAccountId};
use uuid::Uuid;

use super::Revision;

pub const MIN_POLL_INTERVAL_SECS: u32 = 30;

/// `reset_detection_settings.account_scope` 的稳定取值。
///
/// 本层只做不透明存取；轮询 Worker(GUCH-129)消费时按账号状态投影
/// (`AccountStatusProjection`,优先级 disabled → error → quota_exhausted →
/// rate_limited → normal)展开:
///
/// - `all_non_error` = {normal, quota_exhausted, rate_limited}
/// - `normal` = {normal}
/// - `limited` = {quota_exhausted, rate_limited}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetDetectionAccountScope {
    AllNonError,
    Normal,
    Limited,
}

impl ResetDetectionAccountScope {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AllNonError => "all_non_error",
            Self::Normal => "normal",
            Self::Limited => "limited",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "all_non_error" => Some(Self::AllNonError),
            "normal" => Some(Self::Normal),
            "limited" => Some(Self::Limited),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResetDetectionSettings {
    pub enabled: bool,
    pub poll_interval_secs: u32,
    pub account_scope: ResetDetectionAccountScope,
    pub auto_consume_enabled: bool,
    pub updated_at: DateTime<Utc>,
    pub config_revision: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplaceResetDetectionSettings {
    pub enabled: bool,
    pub poll_interval_secs: u32,
    pub account_scope: ResetDetectionAccountScope,
    pub auto_consume_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResetDetectionSettingsMutation {
    pub config_revision: Revision,
    pub settings: ResetDetectionSettings,
}

/// 自动消费开始前持久化的幂等请求；`resumed` 表示复用了未完成请求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResetCreditConsumeReservation {
    pub redeem_request_id: Uuid,
    pub resumed: bool,
}

/// 自动消费确定终态的审计事实；结果未知或额度复核失败时不得写入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompleteResetCreditConsume {
    pub account_id: ProviderAccountId,
    pub redeem_request_id: Uuid,
    pub outcome: ResetCreditConsumeOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResetCreditConsumeOutcome {
    Confirmed {
        provider_code: String,
        quota_status: AccountStatus,
    },
    Rejected {
        provider_code: String,
    },
    Failed {
        error_kind: String,
    },
}
