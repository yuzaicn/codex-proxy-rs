use chrono::{DateTime, Utc};

use super::Revision;

pub const MIN_POLL_INTERVAL_SECS: u32 = 30;

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

#[cfg(test)]
mod tests {
    use super::ResetDetectionAccountScope;

    #[test]
    fn account_scope_values_round_trip_and_reject_unknown_values() {
        for (value, expected) in [
            ("all_non_error", ResetDetectionAccountScope::AllNonError),
            ("normal", ResetDetectionAccountScope::Normal),
            ("limited", ResetDetectionAccountScope::Limited),
        ] {
            assert_eq!(ResetDetectionAccountScope::parse(value), Some(expected));
            assert_eq!(expected.as_str(), value);
        }
        assert_eq!(ResetDetectionAccountScope::parse("other"), None);
    }
}
