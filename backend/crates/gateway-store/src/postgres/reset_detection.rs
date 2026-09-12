use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use gateway_admin::{
    model::{
        MutationContext,
        reset_detection::{
            ReplaceResetDetectionSettings, ResetDetectionAccountScope, ResetDetectionSettings,
            ResetDetectionSettingsMutation,
        },
    },
    ports::store::{AdminStoreResult, ResetDetectionStore},
};

use super::{
    admin_security_audit::append_admin_audit_event_in_transaction,
    runtime_settings::bump_config_revision_in_transaction,
};
use crate::{
    Revision, StoreResult, admin_revision, admin_store_error, mutation_audit, postgres_unavailable,
};
use gateway_admin::model::Revision as AdminRevision;

const ENTITY: &str = "reset detection settings";
type SettingsRow = (bool, i64, String, bool, DateTime<Utc>);

#[derive(Clone)]
pub struct PgResetDetectionStore {
    pool: PgPool,
}

impl PgResetDetectionStore {
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn invalid(message: &str) -> crate::StoreError {
    crate::StoreError::InvalidData {
        entity: ENTITY,
        message: message.to_owned(),
    }
}
fn decode(row: SettingsRow, revision: AdminRevision) -> StoreResult<ResetDetectionSettings> {
    let scope = ResetDetectionAccountScope::parse(&row.2)
        .ok_or_else(|| invalid("unknown account_scope"))?;
    let interval = u32::try_from(row.1).map_err(|_| invalid("invalid poll interval"))?;
    Ok(ResetDetectionSettings {
        enabled: row.0,
        poll_interval_secs: interval,
        account_scope: scope,
        auto_consume_enabled: row.3,
        updated_at: row.4,
        config_revision: revision,
    })
}

async fn revision(pool: &PgPool) -> StoreResult<Revision> {
    let value: i64 =
        sqlx::query_scalar("select config_revision from runtime_settings where id = 1")
            .fetch_one(pool)
            .await
            .map_err(|_| postgres_unavailable("load reset detection revision"))?;
    Revision::new(u64::try_from(value).map_err(|_| invalid("invalid config revision"))?)
}

#[async_trait]
impl ResetDetectionStore for PgResetDetectionStore {
    async fn load_reset_detection_settings(&self) -> AdminStoreResult<ResetDetectionSettings> {
        let row = sqlx::query_as::<_, SettingsRow>("select enabled, poll_interval_secs, account_scope, auto_consume_enabled, updated_at from reset_detection_settings where id = 1")
            .fetch_one(&self.pool).await.map_err(|_| admin_store_error(ENTITY, postgres_unavailable("load reset detection settings")))?;
        let rev = revision(&self.pool)
            .await
            .map_err(|error| admin_store_error(ENTITY, error))?;
        let rev = admin_revision(rev)?;
        decode(row, rev).map_err(|error| admin_store_error(ENTITY, error))
    }

    async fn replace_reset_detection_settings(
        &self,
        command: ReplaceResetDetectionSettings,
        context: &MutationContext,
    ) -> AdminStoreResult<ResetDetectionSettingsMutation> {
        let mut tx = self.pool.begin().await.map_err(|_| {
            admin_store_error(ENTITY, postgres_unavailable("begin reset detection update"))
        })?;
        let result: StoreResult<_> = async {
            let rev = bump_config_revision_in_transaction(&mut tx).await?;
            let row = sqlx::query_as::<_, SettingsRow>("update reset_detection_settings set enabled = $1, poll_interval_secs = $2, account_scope = $3, auto_consume_enabled = $4, updated_at = now() where id = 1 returning enabled, poll_interval_secs, account_scope, auto_consume_enabled, updated_at")
                .bind(command.enabled).bind(i32::try_from(command.poll_interval_secs).map_err(|_| invalid("invalid poll interval"))?)
                .bind(command.account_scope.as_str()).bind(command.auto_consume_enabled).fetch_one(&mut *tx).await.map_err(|_| postgres_unavailable("update reset detection settings"))?;
            let admin_rev = admin_revision(rev).map_err(|_| invalid("invalid config revision"))?;
            append_admin_audit_event_in_transaction(&mut tx, mutation_audit(context, "reset_detection.settings_updated", "reset_detection_settings", "1", vec!["enabled".into(), "poll_interval_secs".into(), "account_scope".into(), "auto_consume_enabled".into()]), rev).await?;
            Ok((admin_rev, decode(row, admin_rev)?))
        }.await;
        match result {
            Ok((revision, settings)) => {
                tx.commit().await.map_err(|_| {
                    admin_store_error(
                        ENTITY,
                        postgres_unavailable("commit reset detection update"),
                    )
                })?;
                Ok(ResetDetectionSettingsMutation {
                    config_revision: revision,
                    settings,
                })
            }
            Err(error) => {
                let _ = tx.rollback().await;
                Err(admin_store_error(ENTITY, error))
            }
        }
    }
}
