use axum::{
    Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use gateway_admin::model::reset_detection::{
    ReplaceResetDetectionSettings, ResetDetectionAccountScope, ResetDetectionSettings,
};
use serde::{Deserialize, Serialize};

use super::{
    AdminAuth, AdminEnvelope, AdminError, AdminJson, AdminResponse, AdminSessionState,
    wire::map_admin_service_error,
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetDetectionSettingsView {
    pub enabled: bool,
    pub poll_interval_secs: u32,
    pub account_scope: String,
    pub auto_consume_enabled: bool,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateResetDetectionSettingsRequest {
    pub enabled: bool,
    pub poll_interval_secs: u32,
    pub account_scope: String,
    pub auto_consume_enabled: bool,
}

impl TryFrom<UpdateResetDetectionSettingsRequest> for ReplaceResetDetectionSettings {
    type Error = AdminError;
    fn try_from(value: UpdateResetDetectionSettingsRequest) -> Result<Self, Self::Error> {
        let account_scope = ResetDetectionAccountScope::parse(&value.account_scope)
            .ok_or_else(|| AdminError::bad_request("accountScope 不合法"))?;
        Ok(Self {
            enabled: value.enabled,
            poll_interval_secs: value.poll_interval_secs,
            account_scope,
            auto_consume_enabled: value.auto_consume_enabled,
        })
    }
}

impl From<ResetDetectionSettings> for ResetDetectionSettingsView {
    fn from(value: ResetDetectionSettings) -> Self {
        Self {
            enabled: value.enabled,
            poll_interval_secs: value.poll_interval_secs,
            account_scope: value.account_scope.as_str().to_owned(),
            auto_consume_enabled: value.auto_consume_enabled,
            updated_at: value.updated_at,
        }
    }
}

pub fn router<S>() -> Router<S>
where
    S: AdminSessionState + Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/api/admin/settings/reset-detection", get(load::<S>))
        .route(
            "/api/admin/settings/reset-detection/update",
            post(update::<S>),
        )
}

async fn load<S>(_auth: AdminAuth, State(state): State<S>) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let settings = state
        .admin_services()
        .reset_detection()
        .load()
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(ResetDetectionSettingsView::from(settings)),
    ))
}

async fn update<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<UpdateResetDetectionSettingsRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let command = request.try_into()?;
    let settings = state
        .admin_services()
        .reset_detection()
        .replace(&auth.context().mutation_context(), command)
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(ResetDetectionSettingsView::from(settings)),
    ))
}
