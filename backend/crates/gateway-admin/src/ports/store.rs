//! 管理控制面所需的持久化能力。
//!
//! 端口按业务资源拆分，方法使用领域模型，不暴露连接池、事务或 Redis client。

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::stream::BoxStream;

use super::backup::BackupStorePorts;
use crate::model::{
    MutationContext, Revision,
    account_groups::{
        AccountGroupListQuery, AccountGroupMemberFact, AccountGroupMutation, AccountGroupPage,
        DeleteAccountGroup, NewAccountGroup, SetAccountGroupEnabled, UpdateAccountGroup,
    },
    accounts::{
        AccountListQuery, AccountPage, AccountPageItem, AccountRuntimeSnapshot,
        AccountUpdateResult, AccountUsage, AccountUsageWindowQuery, AccountUsageWindowResult,
        AccountsUpdateResult, BatchUpdateAccounts, DeleteAccounts, UpdateAccount,
    },
    auth::{AdminAuditEvent, AdminSession},
    client_keys::{
        ClientKeyListQuery, ClientKeyPage, ClientKeyRecord, ClientKeySecret, DeleteClientKey,
        NewClientKey, SetClientKeyEnabled, UpdateClientKey,
    },
    detection::{
        DetectionAccountScope, DetectionConfig, DetectionConfigMutation, DetectionRecord,
        DetectionRecordQuery, DetectionRound, DetectionTarget, NewDetectionRecord,
        ReplaceDetectionConfig,
    },
    observability::{
        DashboardObservation, DashboardRuntimeSlots, DiagnosticDimension, DiagnosticObservation,
        OpsErrorPage, OpsErrorQuery, RequestMetricPoint, TimeRange, UsageCalculatedBillingFact,
        UsageDetail, UsageFilter, UsageOverview, UsagePage, UsageQuery,
    },
    provider_credentials::{
        AuthorizationCommit, CredentialDetails, CredentialImportCommit, CredentialImportResult,
        CredentialMutationResult, CredentialRotationCommit, ProviderExportCredentialInput,
    },
    reset_detection::{
        CompleteResetCreditConsume, ReplaceResetDetectionSettings, ResetCreditConsumeReservation,
        ResetDetectionSettings, ResetDetectionSettingsMutation,
    },
    settings::{AdminApiKey, AdminApiKeyMutation, ReplaceRuntimeSettings, RuntimeSettings},
};

/// 管理端可判定的持久化失败类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminStoreErrorKind {
    Invalid,
    NotFound,
    StaleRevision,
    Conflict,
    Unavailable,
}

/// 隐藏数据库实现细节的持久化错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{resource} store operation failed: {message}")]
pub struct AdminStoreError {
    kind: AdminStoreErrorKind,
    resource: &'static str,
    message: String,
}

impl AdminStoreError {
    #[must_use]
    pub fn new(
        kind: AdminStoreErrorKind,
        resource: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            resource,
            message: message.into(),
        }
    }

    #[must_use]
    pub const fn kind(&self) -> AdminStoreErrorKind {
        self.kind
    }

    #[must_use]
    pub const fn resource(&self) -> &'static str {
        self.resource
    }
}

pub type AdminStoreResult<T> = Result<T, AdminStoreError>;

/// 账号目录与公共账号写操作。
#[async_trait]
pub trait AccountStore: Send + Sync {
    async fn list_accounts(
        &self,
        query: AccountListQuery,
        runtime: AccountRuntimeSnapshot,
    ) -> AdminStoreResult<AccountPage>;

    async fn load_account(
        &self,
        account_id: &str,
        runtime: AccountRuntimeSnapshot,
    ) -> AdminStoreResult<Option<AccountPageItem>>;

    async fn load_account_usage(
        &self,
        range: TimeRange,
        account_ids: &[String],
    ) -> AdminStoreResult<Vec<AccountUsage>>;

    async fn load_account_usage_by_windows(
        &self,
        windows: &[AccountUsageWindowQuery],
    ) -> AdminStoreResult<Vec<AccountUsageWindowResult>>;

    async fn credential_details(
        &self,
        provider_kind: &gateway_core::routing::ProviderKind,
        account_id: &gateway_core::account::ProviderAccountId,
    ) -> AdminStoreResult<Option<CredentialDetails>>;

    async fn load_credentials_for_export(
        &self,
        provider_kind: &gateway_core::routing::ProviderKind,
        account_ids: &[gateway_core::account::ProviderAccountId],
    ) -> AdminStoreResult<Vec<ProviderExportCredentialInput>>;

    async fn commit_credential_import(
        &self,
        command: CredentialImportCommit,
        context: &MutationContext,
    ) -> AdminStoreResult<CredentialImportResult>;

    async fn commit_authorization(
        &self,
        command: AuthorizationCommit,
        context: &MutationContext,
    ) -> AdminStoreResult<CredentialMutationResult>;

    async fn commit_credential_rotation(
        &self,
        command: CredentialRotationCommit,
        context: &MutationContext,
    ) -> AdminStoreResult<CredentialMutationResult>;

    async fn commit_credential_refresh(
        &self,
        command: CredentialRotationCommit,
        context: &MutationContext,
    ) -> AdminStoreResult<CredentialMutationResult>;

    async fn update_account(
        &self,
        command: UpdateAccount,
        context: &MutationContext,
    ) -> AdminStoreResult<AccountUpdateResult>;

    async fn recover_account(
        &self,
        account_id: &gateway_core::account::ProviderAccountId,
        context: &MutationContext,
    ) -> AdminStoreResult<AccountUpdateResult>;

    /// 写入调度暂停事实并记录审计：`suspension` 为 `Some` 时暂停并记录来源，
    /// 为 `None` 时恢复调度并清空来源。管理侧仅此一个写入口。
    async fn set_scheduling_suspended(
        &self,
        account_id: &gateway_core::account::ProviderAccountId,
        suspension: Option<gateway_core::account::SchedulingSuspensionSource>,
        context: &MutationContext,
    ) -> AdminStoreResult<AccountUpdateResult>;

    async fn batch_update_accounts(
        &self,
        command: BatchUpdateAccounts,
        context: &MutationContext,
    ) -> AdminStoreResult<AccountsUpdateResult>;

    async fn delete_accounts(
        &self,
        command: DeleteAccounts,
        context: &MutationContext,
    ) -> AdminStoreResult<Revision>;

    async fn record_credential_export(
        &self,
        account_ids: &[gateway_core::account::ProviderAccountId],
        context: &MutationContext,
    ) -> AdminStoreResult<()>;
}

/// 可丢失账号运行态的管理读端口；跨存储编排由 Admin application service 拥有。
#[async_trait]
pub trait AccountRuntimeStore: Send + Sync {
    async fn active_rate_limits(&self) -> AdminStoreResult<AccountRuntimeSnapshot>;

    async fn account_runtime(
        &self,
        account_ids: &[String],
    ) -> AdminStoreResult<AccountRuntimeSnapshot>;
}

/// 管理员密码、会话和安全审计。
#[async_trait]
pub trait AuthStore: Send + Sync {
    async fn load_password_hash(&self, admin_user_id: &str) -> AdminStoreResult<Option<String>>;

    async fn create_password_hash_if_absent(
        &self,
        admin_user_id: &str,
        password_hash: &str,
    ) -> AdminStoreResult<bool>;

    async fn load_admin_api_key(&self) -> AdminStoreResult<Option<AdminApiKey>>;

    async fn load_session(&self, session_id: &str) -> AdminStoreResult<Option<AdminSession>>;

    async fn store_session(&self, session_id: &str, session: &AdminSession)
    -> AdminStoreResult<()>;

    async fn delete_session(&self, session_id: &str) -> AdminStoreResult<Option<AdminSession>>;

    async fn append_audit_event(&self, event: AdminAuditEvent) -> AdminStoreResult<()>;
}

/// Client API Key 管理写入。
#[async_trait]
pub trait ClientKeyStore: Send + Sync {
    async fn list_client_keys(&self, query: ClientKeyListQuery) -> AdminStoreResult<ClientKeyPage>;

    async fn reveal_client_key(
        &self,
        id: &gateway_core::policy::ClientApiKeyId,
    ) -> AdminStoreResult<Option<ClientKeySecret>>;

    async fn create_client_key(
        &self,
        command: NewClientKey,
        context: &MutationContext,
    ) -> AdminStoreResult<(Revision, ClientKeyRecord)>;

    async fn update_client_key(
        &self,
        command: UpdateClientKey,
        context: &MutationContext,
    ) -> AdminStoreResult<(Revision, ClientKeyRecord)>;

    async fn set_client_key_enabled(
        &self,
        command: SetClientKeyEnabled,
        context: &MutationContext,
    ) -> AdminStoreResult<(Revision, ClientKeyRecord)>;

    async fn delete_client_key(
        &self,
        command: DeleteClientKey,
        context: &MutationContext,
    ) -> AdminStoreResult<Revision>;
}

/// Provider-neutral account group management transactions.
#[async_trait]
pub trait AccountGroupStore: Send + Sync {
    async fn list_account_groups(
        &self,
        query: AccountGroupListQuery,
    ) -> AdminStoreResult<AccountGroupPage>;

    async fn load_account_group_members(
        &self,
        group_ids: &[gateway_core::routing::AccountGroupId],
    ) -> AdminStoreResult<Vec<AccountGroupMemberFact>>;

    async fn create_account_group(
        &self,
        command: NewAccountGroup,
        context: &MutationContext,
    ) -> AdminStoreResult<AccountGroupMutation>;

    async fn update_account_group(
        &self,
        command: UpdateAccountGroup,
        context: &MutationContext,
    ) -> AdminStoreResult<AccountGroupMutation>;

    async fn set_account_group_enabled(
        &self,
        command: SetAccountGroupEnabled,
        context: &MutationContext,
    ) -> AdminStoreResult<AccountGroupMutation>;

    async fn delete_account_group(
        &self,
        command: DeleteAccountGroup,
        context: &MutationContext,
    ) -> AdminStoreResult<AccountGroupMutation>;
}

/// 逐条读取的已计算费用事实；消费结束或丢弃时释放查询资源。
pub type UsageCalculatedBillingStream<'a> =
    BoxStream<'a, AdminStoreResult<UsageCalculatedBillingFact>>;

/// 用量、趋势、诊断与运维错误的只读能力。
#[async_trait]
pub trait ObservabilityStore: Send + Sync {
    /// 返回历史统计区间和指定观测时刻下的实时账号状态。
    async fn dashboard_summary(
        &self,
        range: TimeRange,
        observed_at: DateTime<Utc>,
    ) -> AdminStoreResult<DashboardObservation>;

    /// 返回 Dashboard 可选的实时槽位事实。
    ///
    /// 该状态来自可丢失的运行时存储；无实现或运行时存储不可用时返回 `None`，不影响
    /// 持久观测数据的读取。
    async fn dashboard_runtime_slots(
        &self,
        _observed_at: DateTime<Utc>,
    ) -> AdminStoreResult<Option<DashboardRuntimeSlots>> {
        Ok(None)
    }

    async fn dashboard_trend(&self, range: TimeRange) -> AdminStoreResult<Vec<RequestMetricPoint>>;

    async fn usage_trend(
        &self,
        range: TimeRange,
        filter: UsageFilter,
    ) -> AdminStoreResult<Vec<RequestMetricPoint>>;

    /// 流式返回可由 Provider 重新校验的已计算费用事实，不保证顺序。
    /// 查询及解码错误由流返回；调用方应逐条聚合，避免收集整个区间。
    fn usage_calculated_billing_facts(
        &self,
        range: TimeRange,
        filter: UsageFilter,
    ) -> UsageCalculatedBillingStream<'_>;

    async fn list_usage_records(&self, query: UsageQuery) -> AdminStoreResult<UsagePage>;

    async fn usage_record_detail(&self, request_id: &str) -> AdminStoreResult<UsageDetail>;

    async fn usage_summary(
        &self,
        range: TimeRange,
        filter: UsageFilter,
    ) -> AdminStoreResult<UsageOverview>;

    async fn usage_diagnostics(
        &self,
        range: TimeRange,
        filter: UsageFilter,
        dimension: DiagnosticDimension,
    ) -> AdminStoreResult<Vec<DiagnosticObservation>>;

    async fn list_ops_errors(&self, query: OpsErrorQuery) -> AdminStoreResult<OpsErrorPage>;
}

/// 降智检测配置与检测记录读写。
#[async_trait]
pub trait DetectionStore: Send + Sync {
    /// 读取全局检测配置；配置行尚未写入时返回 `None`。
    async fn load_detection_config(&self) -> AdminStoreResult<Option<DetectionConfig>>;

    async fn replace_detection_config(
        &self,
        command: ReplaceDetectionConfig,
        context: &MutationContext,
    ) -> AdminStoreResult<DetectionConfigMutation>;

    /// 分页读取检测记录（含账号身份投影），按检测时间倒排。
    async fn list_detection_records(
        &self,
        query: DetectionRecordQuery,
    ) -> AdminStoreResult<Vec<DetectionRecord>>;

    /// 读取最近的检测批次聚合，按检测时间倒排。
    async fn list_detection_rounds(&self, limit: u32) -> AdminStoreResult<Vec<DetectionRound>>;

    /// 读取最近一条检测记录的落库时间；没有成功落库的轮次时返回 `None`。
    async fn latest_detection_checked_at(&self) -> AdminStoreResult<Option<DateTime<Utc>>>;

    /// 检测 Worker 按账号范围读取待探测账号及其调度暂停事实。
    async fn list_detection_targets(
        &self,
        scope: &DetectionAccountScope,
    ) -> AdminStoreResult<Vec<DetectionTarget>>;

    /// 读取单条检测记录的 HTML 原文；记录不存在或 `html_content` 为 NULL 时返回 `None`。
    async fn load_detection_record_html(&self, record_id: i64) -> AdminStoreResult<Option<String>>;

    /// 检测 Worker 追加一条检测记录；观测数据不参与配置 revision 与审计。
    async fn insert_detection_record(&self, record: NewDetectionRecord) -> AdminStoreResult<()>;
}

/// Runtime settings 与管理员 API Key 写入。
#[async_trait]
pub trait SettingsStore: Send + Sync {
    async fn load_runtime_settings(&self) -> AdminStoreResult<RuntimeSettings>;

    async fn admin_api_key_exists(&self) -> AdminStoreResult<bool>;

    async fn replace_runtime_settings(
        &self,
        command: ReplaceRuntimeSettings,
        context: &MutationContext,
    ) -> AdminStoreResult<RuntimeSettings>;

    async fn replace_admin_api_key(
        &self,
        key: AdminApiKey,
        context: &MutationContext,
    ) -> AdminStoreResult<AdminApiKeyMutation>;

    async fn delete_admin_api_key(
        &self,
        context: &MutationContext,
    ) -> AdminStoreResult<AdminApiKeyMutation>;
}

#[async_trait]
pub trait ResetDetectionStore: Send + Sync {
    async fn load_reset_detection_settings(&self) -> AdminStoreResult<ResetDetectionSettings>;

    /// 读取最近一条重置卡观测时间，用于跨实例/重启后的轮次间隔判断。
    async fn latest_reset_detection_observed_at(&self) -> AdminStoreResult<Option<DateTime<Utc>>>;

    async fn replace_reset_detection_settings(
        &self,
        command: ReplaceResetDetectionSettings,
        context: &MutationContext,
    ) -> AdminStoreResult<ResetDetectionSettingsMutation>;

    /// 写回上游重置卡数量观测；不推进配置 revision。
    async fn record_reset_credits_observation(
        &self,
        account_id: &gateway_core::account::ProviderAccountId,
        available_count: u64,
        observed_at: DateTime<Utc>,
    ) -> AdminStoreResult<()>;

    /// 在不可逆调用前持久化 UUIDv4；账号已有未完成请求时固定复用旧值。
    async fn reserve_reset_credit_consume(
        &self,
        account_id: &gateway_core::account::ProviderAccountId,
        candidate: uuid::Uuid,
    ) -> AdminStoreResult<ResetCreditConsumeReservation>;

    /// 仅在消费结果确定时结束幂等请求；歧义状态必须保持未完成。
    async fn complete_reset_credit_consume(
        &self,
        completion: CompleteResetCreditConsume,
    ) -> AdminStoreResult<()>;
}

struct UnavailableResetDetectionStore;

#[async_trait]
impl ResetDetectionStore for UnavailableResetDetectionStore {
    async fn load_reset_detection_settings(&self) -> AdminStoreResult<ResetDetectionSettings> {
        Err(AdminStoreError::new(
            AdminStoreErrorKind::Unavailable,
            "reset detection settings",
            "store is unavailable",
        ))
    }

    async fn latest_reset_detection_observed_at(&self) -> AdminStoreResult<Option<DateTime<Utc>>> {
        Err(AdminStoreError::new(
            AdminStoreErrorKind::Unavailable,
            "latest reset detection timestamp",
            "store is unavailable",
        ))
    }

    async fn replace_reset_detection_settings(
        &self,
        _command: ReplaceResetDetectionSettings,
        _context: &MutationContext,
    ) -> AdminStoreResult<ResetDetectionSettingsMutation> {
        Err(AdminStoreError::new(
            AdminStoreErrorKind::Unavailable,
            "reset detection settings",
            "store is unavailable",
        ))
    }

    async fn record_reset_credits_observation(
        &self,
        _account_id: &gateway_core::account::ProviderAccountId,
        _available_count: u64,
        _observed_at: DateTime<Utc>,
    ) -> AdminStoreResult<()> {
        Err(AdminStoreError::new(
            AdminStoreErrorKind::Unavailable,
            "reset credits observation",
            "store is unavailable",
        ))
    }

    async fn reserve_reset_credit_consume(
        &self,
        _account_id: &gateway_core::account::ProviderAccountId,
        _candidate: uuid::Uuid,
    ) -> AdminStoreResult<ResetCreditConsumeReservation> {
        Err(AdminStoreError::new(
            AdminStoreErrorKind::Unavailable,
            "reset credit consume reservation",
            "store is unavailable",
        ))
    }

    async fn complete_reset_credit_consume(
        &self,
        _completion: CompleteResetCreditConsume,
    ) -> AdminStoreResult<()> {
        Err(AdminStoreError::new(
            AdminStoreErrorKind::Unavailable,
            "reset credit consume completion",
            "store is unavailable",
        ))
    }
}

/// 账号目录、运行态与分组所需的 Store 能力集合。
#[derive(Clone)]
pub struct AdminAccountStorePorts {
    accounts: Arc<dyn AccountStore>,
    runtime: Arc<dyn AccountRuntimeStore>,
    groups: Arc<dyn AccountGroupStore>,
    proxies: Arc<dyn super::proxy::ProxyStore>,
}

impl AdminAccountStorePorts {
    #[must_use]
    pub fn new(
        accounts: Arc<dyn AccountStore>,
        runtime: Arc<dyn AccountRuntimeStore>,
        groups: Arc<dyn AccountGroupStore>,
        proxies: Arc<dyn super::proxy::ProxyStore>,
    ) -> Self {
        Self {
            accounts,
            runtime,
            groups,
            proxies,
        }
    }
}

/// 管理用例所需能力的封闭集合。
///
/// 字段保持私有，每个 getter 只交出一种明确能力。该类型不提供通用拆包入口。
#[derive(Clone)]
pub struct AdminStorePorts {
    accounts: AdminAccountStorePorts,
    auth: Arc<dyn AuthStore>,
    client_keys: Arc<dyn ClientKeyStore>,
    observability: Arc<dyn ObservabilityStore>,
    settings: Arc<dyn SettingsStore>,
    detection: Arc<dyn DetectionStore>,
    reset_detection: Arc<dyn ResetDetectionStore>,
    backup: BackupStorePorts,
}

impl AdminStorePorts {
    #[must_use]
    pub fn new(
        accounts: AdminAccountStorePorts,
        auth: Arc<dyn AuthStore>,
        client_keys: Arc<dyn ClientKeyStore>,
        observability: Arc<dyn ObservabilityStore>,
        settings: Arc<dyn SettingsStore>,
        detection: Arc<dyn DetectionStore>,
        backup: BackupStorePorts,
    ) -> Self {
        Self {
            accounts,
            auth,
            client_keys,
            observability,
            settings,
            detection,
            reset_detection: Arc::new(UnavailableResetDetectionStore),
            backup,
        }
    }

    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_reset_detection(
        accounts: AdminAccountStorePorts,
        auth: Arc<dyn AuthStore>,
        client_keys: Arc<dyn ClientKeyStore>,
        observability: Arc<dyn ObservabilityStore>,
        settings: Arc<dyn SettingsStore>,
        detection: Arc<dyn DetectionStore>,
        reset_detection: Arc<dyn ResetDetectionStore>,
        backup: BackupStorePorts,
    ) -> Self {
        let mut ports = Self::new(
            accounts,
            auth,
            client_keys,
            observability,
            settings,
            detection,
            backup,
        );
        ports.reset_detection = reset_detection;
        ports
    }

    #[must_use]
    pub fn accounts(&self) -> Arc<dyn AccountStore> {
        self.accounts.accounts.clone()
    }

    #[must_use]
    pub fn account_runtime(&self) -> Arc<dyn AccountRuntimeStore> {
        self.accounts.runtime.clone()
    }

    #[must_use]
    pub fn account_groups(&self) -> Arc<dyn AccountGroupStore> {
        self.accounts.groups.clone()
    }

    #[must_use]
    pub fn proxies(&self) -> Arc<dyn super::proxy::ProxyStore> {
        self.accounts.proxies.clone()
    }

    #[must_use]
    pub fn auth(&self) -> Arc<dyn AuthStore> {
        self.auth.clone()
    }

    #[must_use]
    pub fn client_keys(&self) -> Arc<dyn ClientKeyStore> {
        self.client_keys.clone()
    }

    #[must_use]
    pub fn observability(&self) -> Arc<dyn ObservabilityStore> {
        self.observability.clone()
    }

    #[must_use]
    pub fn settings(&self) -> Arc<dyn SettingsStore> {
        self.settings.clone()
    }

    #[must_use]
    pub fn detection(&self) -> Arc<dyn DetectionStore> {
        self.detection.clone()
    }

    #[must_use]
    pub fn reset_detection(&self) -> Arc<dyn ResetDetectionStore> {
        self.reset_detection.clone()
    }

    #[must_use]
    pub fn backup(&self) -> BackupStorePorts {
        self.backup.clone()
    }
}
