//! 降智检测配置与检测记录的管理端 HTTP 合同与路由。
//!
//! 该组端点的 wire 形状与前端 `detection.ts` 锁定为 snake_case，
//! 记录与批次接口直接返回数组，不套分页信封。

use axum::{
    Router,
    body::Body,
    extract::State,
    http::{HeaderName, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::{DateTime, Utc};
use gateway_admin::model::detection::{
    DetectionAccountScope, DetectionConfig, DetectionRecord, DetectionRecordQuery, DetectionRound,
    ReplaceDetectionConfig,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    AdminAuth, AdminEnvelope, AdminError, AdminJson, AdminQuery, AdminResponse, AdminSessionState,
    WireValidationError, wire::map_admin_service_error,
};

const DEFAULT_RECORD_PAGE_SIZE: u32 = 200;
const MAX_RECORD_PAGE_SIZE: u32 = 500;

/// `account_scope` 的线上形状：`{"all": true}` 或 `{"account_ids": [...]}`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DetectionScopeWire {
    All { all: bool },
    Selected { account_ids: Vec<String> },
}

impl DetectionScopeWire {
    fn into_domain(self) -> Result<DetectionAccountScope, WireValidationError> {
        match self {
            Self::All { all: true } => Ok(DetectionAccountScope::AllAccounts),
            Self::All { all: false } => Err(WireValidationError::new("account_scope")),
            Self::Selected { account_ids } => {
                Ok(DetectionAccountScope::SelectedAccounts { account_ids })
            }
        }
    }
}

impl From<DetectionAccountScope> for DetectionScopeWire {
    fn from(scope: DetectionAccountScope) -> Self {
        match scope {
            DetectionAccountScope::AllAccounts => Self::All { all: true },
            DetectionAccountScope::SelectedAccounts { account_ids } => {
                Self::Selected { account_ids }
            }
        }
    }
}

/// 检测配置视图。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DetectionConfigView {
    pub enabled: bool,
    pub account_scope: DetectionScopeWire,
    pub interval_secs: u32,
    pub model: String,
}

impl From<DetectionConfig> for DetectionConfigView {
    fn from(config: DetectionConfig) -> Self {
        Self {
            enabled: config.enabled,
            account_scope: config.account_scope.into(),
            interval_secs: config.interval_secs,
            model: config.model,
        }
    }
}

/// 写入全局检测配置的请求。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateDetectionConfigRequest {
    pub enabled: bool,
    pub account_scope: DetectionScopeWire,
    pub interval_secs: u32,
    pub model: String,
}

impl UpdateDetectionConfigRequest {
    fn into_command(self) -> Result<ReplaceDetectionConfig, WireValidationError> {
        Ok(ReplaceDetectionConfig {
            enabled: self.enabled,
            account_scope: self.account_scope.into_domain()?,
            interval_secs: self.interval_secs,
            model: self.model,
        })
    }
}

/// 检测记录 HTML 查询；管理端冻结为静态路径 + 查询参数，不使用路径参数。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DetectionRecordHtmlQuery {
    pub id: Option<i64>,
}

impl DetectionRecordHtmlQuery {
    /// 校验记录 ID：必填且为正整数。
    pub fn validate(&self) -> Result<(), WireValidationError> {
        match self.id {
            Some(id) if id > 0 => Ok(()),
            _ => Err(WireValidationError::new("id")),
        }
    }

    fn into_id(self) -> Result<i64, WireValidationError> {
        self.validate()?;
        self.id.ok_or_else(|| WireValidationError::new("id"))
    }
}

/// 检测记录查询；`detection_round_id` 过滤可选，分页参数缺省为第一页。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DetectionRecordsQuery {
    pub detection_round_id: Option<String>,
    pub page: Option<u32>,
    pub page_size: Option<u32>,
}

impl DetectionRecordsQuery {
    fn into_query(self) -> Result<DetectionRecordQuery, WireValidationError> {
        let detection_round_id = self
            .detection_round_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| {
                Uuid::parse_str(value).map_err(|_| WireValidationError::new("detection_round_id"))
            })
            .transpose()?;
        let page = self.page.unwrap_or(1);
        if page == 0 {
            return Err(WireValidationError::new("page"));
        }
        let page_size = self.page_size.unwrap_or(DEFAULT_RECORD_PAGE_SIZE);
        if page_size == 0 || page_size > MAX_RECORD_PAGE_SIZE {
            return Err(WireValidationError::new("page_size"));
        }
        Ok(DetectionRecordQuery {
            detection_round_id,
            page,
            page_size,
        })
    }
}

/// 一条检测记录视图（含账号身份投影）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DetectionRecordView {
    pub id: i64,
    pub detection_round_id: String,
    pub account_id: String,
    pub account_email: Option<String>,
    pub account_name: Option<String>,
    pub account_plan_type: Option<String>,
    pub account_plan_type_display: Option<String>,
    pub checked_at: DateTime<Utc>,
    pub degraded: bool,
    pub scheduling_suspended: bool,
    pub reasoning_content: Option<String>,
    pub prompt_used: Option<String>,
}

impl From<DetectionRecord> for DetectionRecordView {
    fn from(record: DetectionRecord) -> Self {
        Self {
            id: record.id,
            detection_round_id: record.detection_round_id.to_string(),
            account_id: record.account_id,
            account_email: record.account_email,
            account_name: record.account_name,
            account_plan_type: record.account_plan_type,
            account_plan_type_display: record.account_plan_type_display,
            checked_at: record.checked_at,
            degraded: record.degraded,
            scheduling_suspended: record.scheduling_suspended,
            reasoning_content: record.reasoning_content,
            prompt_used: record.prompt_used,
        }
    }
}

/// 一个检测批次视图。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DetectionRoundView {
    pub detection_round_id: String,
    pub checked_at: DateTime<Utc>,
    pub degraded_count: u64,
    pub normal_count: u64,
}

impl From<DetectionRound> for DetectionRoundView {
    fn from(round: DetectionRound) -> Self {
        Self {
            detection_round_id: round.detection_round_id.to_string(),
            checked_at: round.checked_at,
            degraded_count: round.degraded_count,
            normal_count: round.normal_count,
        }
    }
}

/// 构造固定的检测配置与检测记录路由。
pub fn router<S>() -> Router<S>
where
    S: AdminSessionState + Clone + Send + Sync + 'static,
{
    Router::new()
        .route(
            "/api/admin/detection/config",
            get(detection_config::<S>).post(update_detection_config::<S>),
        )
        .route("/api/admin/detection/records", get(detection_records::<S>))
        .route(
            "/api/admin/detection/records/rounds",
            get(detection_rounds::<S>),
        )
        .route(
            "/api/admin/detection/records/html",
            get(detection_record_html::<S>),
        )
}

async fn detection_config<S>(
    _auth: AdminAuth,
    State(state): State<S>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let config = state
        .admin_services()
        .detection()
        .config()
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(DetectionConfigView::from(config)),
    ))
}

async fn update_detection_config<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<UpdateDetectionConfigRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let command = request.into_command().map_err(map_wire_error)?;
    let config = state
        .admin_services()
        .detection()
        .replace_config(&auth.context().mutation_context(), command)
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(DetectionConfigView::from(config)),
    ))
}

async fn detection_records<S>(
    _auth: AdminAuth,
    State(state): State<S>,
    AdminQuery(query): AdminQuery<DetectionRecordsQuery>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let query = query.into_query().map_err(map_wire_error)?;
    let records = state
        .admin_services()
        .detection()
        .records(query)
        .await
        .map_err(map_admin_service_error)?;
    let views = records
        .into_iter()
        .map(DetectionRecordView::from)
        .collect::<Vec<_>>();
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(views)))
}

async fn detection_rounds<S>(
    _auth: AdminAuth,
    State(state): State<S>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let rounds = state
        .admin_services()
        .detection()
        .rounds()
        .await
        .map_err(map_admin_service_error)?;
    let views = rounds
        .into_iter()
        .map(DetectionRoundView::from)
        .collect::<Vec<_>>();
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(views)))
}

async fn detection_record_html<S>(
    _auth: AdminAuth,
    State(state): State<S>,
    AdminQuery(query): AdminQuery<DetectionRecordHtmlQuery>,
) -> Result<Response, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let record_id = query.into_id().map_err(map_wire_error)?;
    let html = state
        .admin_services()
        .detection()
        .record_html(record_id)
        .await
        .map_err(map_admin_service_error)?;
    Ok(detection_record_html_response(html))
}

/// 将不受信的 AI 生成 HTML 投影为沙箱化同源响应。
///
/// `Content-Security-Policy: sandbox allow-scripts` 让文档无论在前端 iframe 里还是被
/// 管理员直接打开都运行在 opaque origin：脚本可以执行（保留生成动画），但拿不到
/// 管理端源的 Cookie、存储与 DOM，封死存储型 XSS；`nosniff` 防止类型嗅探绕过。
#[must_use]
pub fn detection_record_html_response(html: String) -> Response {
    let mut response = Response::new(Body::from(html));
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    headers.insert(
        HeaderName::from_static("content-security-policy"),
        HeaderValue::from_static("sandbox allow-scripts"),
    );
    headers.insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        HeaderName::from_static("cross-origin-resource-policy"),
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response
}

fn map_wire_error(error: WireValidationError) -> AdminError {
    AdminError::bad_request(format!("{} 字段不合法", error.field()))
}
