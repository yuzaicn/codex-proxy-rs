use async_trait::async_trait;
use chrono::{DateTime, Utc};
use gateway_core::account::ProviderAccountId;
use sqlx::PgPool;
use uuid::{Uuid, Version};

use gateway_admin::{
    model::{
        MutationContext,
        reset_detection::{
            CompleteResetCreditConsume, ReplaceResetDetectionSettings, ResetCreditConsumeOutcome,
            ResetCreditConsumeReservation, ResetDetectionAccountScope, ResetDetectionSettings,
            ResetDetectionSettingsMutation,
        },
    },
    ports::store::{AdminStoreError, AdminStoreResult, ResetDetectionStore},
};

use super::{
    admin_security_audit::{
        append_admin_audit_event_in_transaction,
        append_admin_audit_event_without_revision_in_transaction,
        load_unfinished_audit_request_in_transaction,
    },
    runtime_settings::bump_config_revision_in_transaction,
};
use crate::{
    Revision, StoreResult, admin_revision, admin_store_error, mutation_audit, postgres_unavailable,
};
use gateway_admin::model::Revision as AdminRevision;

const ENTITY: &str = "reset detection settings";
const RESET_CREDIT_ENTITY: &str = "provider_account";
const AUTO_CONSUME_STARTED: &str = "reset_detection.auto_consume_started";
const AUTO_CONSUME_FINISHED: &str = "reset_detection.auto_consume_finished";
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

    async fn latest_reset_detection_observed_at(&self) -> AdminStoreResult<Option<DateTime<Utc>>> {
        sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
            "select max(reset_credits_observed_at) from provider_accounts",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|_| {
            admin_store_error(
                "reset credits observation",
                postgres_unavailable("load latest reset credits observation"),
            )
        })
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

    async fn record_reset_credits_observation(
        &self,
        account_id: &ProviderAccountId,
        available_count: u64,
        observed_at: DateTime<Utc>,
    ) -> AdminStoreResult<()> {
        let available_count = i64::try_from(available_count).map_err(|_| {
            admin_store_error(
                "reset credits observation",
                invalid("available count exceeds PostgreSQL bigint"),
            )
        })?;
        let result = sqlx::query(
            "update provider_accounts
                set reset_credits_available_count = $2,
                    reset_credits_observed_at = $3,
                    updated_at = greatest(updated_at, $3)
              where id = $1",
        )
        .bind(account_id.as_str())
        .bind(available_count)
        .bind(observed_at)
        .execute(&self.pool)
        .await
        .map_err(|_| {
            admin_store_error(
                "reset credits observation",
                postgres_unavailable("record reset credits observation"),
            )
        })?;
        if result.rows_affected() == 1 {
            Ok(())
        } else {
            Err(AdminStoreError::new(
                gateway_admin::ports::store::AdminStoreErrorKind::NotFound,
                "reset credits observation",
                "provider account does not exist",
            ))
        }
    }

    async fn reserve_reset_credit_consume(
        &self,
        account_id: &ProviderAccountId,
        candidate: Uuid,
    ) -> AdminStoreResult<ResetCreditConsumeReservation> {
        if candidate.get_version() != Some(Version::Random) {
            return Err(AdminStoreError::new(
                gateway_admin::ports::store::AdminStoreErrorKind::Invalid,
                "reset credit consume reservation",
                "candidate must be UUIDv4",
            ));
        }
        let mut transaction = self.pool.begin().await.map_err(|_| {
            admin_store_error(
                "reset credit consume reservation",
                postgres_unavailable("begin reset credit consume reservation"),
            )
        })?;
        let result: StoreResult<_> = async {
            sqlx::query("select pg_advisory_xact_lock(hashtextextended($1, 0))")
                .bind(format!("reset_detection:{}", account_id.as_str()))
                .execute(&mut *transaction)
                .await
                .map_err(|_| postgres_unavailable("lock reset credit consume reservation"))?;
            if let Some(persisted) = load_unfinished_audit_request_in_transaction(
                &mut transaction,
                AUTO_CONSUME_STARTED,
                AUTO_CONSUME_FINISHED,
                RESET_CREDIT_ENTITY,
                account_id.as_str(),
            )
            .await?
            {
                let redeem_request_id = Uuid::parse_str(&persisted)
                    .map_err(|_| invalid("persisted redeem request ID is invalid"))?;
                if redeem_request_id.get_version() != Some(Version::Random)
                    || redeem_request_id.hyphenated().to_string() != persisted
                {
                    return Err(invalid(
                        "persisted redeem request ID is not canonical UUIDv4",
                    ));
                }
                return Ok(ResetCreditConsumeReservation {
                    redeem_request_id,
                    resumed: true,
                });
            }
            let context = MutationContext {
                actor: gateway_admin::model::MutationActor::System,
                request_id: candidate.hyphenated().to_string(),
            };
            let event = mutation_audit(
                &context,
                AUTO_CONSUME_STARTED,
                RESET_CREDIT_ENTITY,
                account_id.as_str(),
                vec!["redeem_request_id".to_owned()],
            );
            append_admin_audit_event_without_revision_in_transaction(&mut transaction, event)
                .await?;
            Ok(ResetCreditConsumeReservation {
                redeem_request_id: candidate,
                resumed: false,
            })
        }
        .await;
        match result {
            Ok(reservation) => {
                transaction.commit().await.map_err(|_| {
                    admin_store_error(
                        "reset credit consume reservation",
                        postgres_unavailable("commit reset credit consume reservation"),
                    )
                })?;
                Ok(reservation)
            }
            Err(error) => {
                let _ = transaction.rollback().await;
                Err(admin_store_error("reset credit consume reservation", error))
            }
        }
    }

    async fn complete_reset_credit_consume(
        &self,
        completion: CompleteResetCreditConsume,
    ) -> AdminStoreResult<()> {
        let context = MutationContext {
            actor: gateway_admin::model::MutationActor::System,
            request_id: completion.redeem_request_id.hyphenated().to_string(),
        };
        let changed_fields = match completion.outcome {
            ResetCreditConsumeOutcome::Confirmed {
                provider_code,
                quota_status,
            } => vec![
                "outcome:confirmed".to_owned(),
                format!("provider_code:{provider_code}"),
                format!("quota_status:{}", quota_status.as_str()),
            ],
            ResetCreditConsumeOutcome::Rejected { provider_code } => vec![
                "outcome:rejected".to_owned(),
                format!("provider_code:{provider_code}"),
            ],
            ResetCreditConsumeOutcome::Failed { error_kind } => vec![
                "outcome:failed".to_owned(),
                format!("error_kind:{error_kind}"),
            ],
        };
        let event = mutation_audit(
            &context,
            AUTO_CONSUME_FINISHED,
            RESET_CREDIT_ENTITY,
            completion.account_id.as_str(),
            changed_fields,
        );
        super::admin_security_audit::AdminSecurityAuditRepository::append_admin_audit_event(
            &super::admin_security_audit::PgAdminSecurityAuditRepository::new(self.pool.clone()),
            event,
        )
        .await
        .map_err(|error| admin_store_error("reset credit consume completion", error))
    }
}
