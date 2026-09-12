//! 降智检测配置与检测记录的 PostgreSQL owner。

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use gateway_admin::{
    model::{
        MutationContext,
        detection::{
            DetectionAccountScope, DetectionConfig, DetectionConfigMutation, DetectionRecord,
            DetectionRecordQuery, DetectionRound, DetectionTarget, NewDetectionRecord,
            ReplaceDetectionConfig,
        },
    },
    ports::store::{AdminStoreResult, DetectionStore},
};
use gateway_core::{account::SchedulingSuspensionSource, routing::ProviderKind};

use crate::{
    Revision, StoreError, StoreResult, admin_revision, admin_store_error, mutation_audit,
    postgres_unavailable,
};

use super::admin_security_audit::append_admin_audit_event_in_transaction;
use super::runtime_settings::bump_config_revision_in_transaction;

const ENTITY: &str = "intelligence detection";

type ConfigRow = (bool, serde_json::Value, i32, String, DateTime<Utc>);
type RecordRow = (
    i64,
    String,
    String,
    Option<String>,
    String,
    String,
    Option<String>,
    bool,
    DateTime<Utc>,
    bool,
);
type RoundRow = (String, DateTime<Utc>, i64, i64);
type TargetRow = (String, String, bool, Option<String>);

/// 降智检测配置与检测记录的 PostgreSQL adapter。
#[derive(Clone)]
pub struct PgDetectionStore {
    pool: PgPool,
}

impl PgDetectionStore {
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn load_config(&self) -> StoreResult<Option<DetectionConfig>> {
        let row = sqlx::query_as::<_, ConfigRow>(
            "select enabled, account_scope, interval_secs, model, updated_at
             from intelligence_detection_configs order by id limit 1",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| postgres_unavailable("load detection config"))?;
        row.map(detection_config_from_row).transpose()
    }

    async fn replace_config(
        &self,
        command: ReplaceDetectionConfig,
        audit: super::AdminAuditEvent,
    ) -> StoreResult<(Revision, DetectionConfig)> {
        let scope = account_scope_json(&command.account_scope);
        let interval_secs =
            i32::try_from(command.interval_secs).map_err(|_| invalid("invalid interval"))?;
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| postgres_unavailable("begin detection config replace"))?;
        let result = async {
            // 先推进 runtime_settings 单行的 config revision：该行锁同时把并发的
            // 配置替换串行化，避免空表时的双插入。
            let revision = bump_config_revision_in_transaction(&mut transaction).await?;
            let updated = sqlx::query_as::<_, ConfigRow>(
                "update intelligence_detection_configs
                 set enabled = $1, account_scope = $2, interval_secs = $3, model = $4,
                     updated_at = now()
                 where id = (select id from intelligence_detection_configs order by id limit 1)
                 returning enabled, account_scope, interval_secs, model, updated_at",
            )
            .bind(command.enabled)
            .bind(&scope)
            .bind(interval_secs)
            .bind(&command.model)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|_| postgres_unavailable("update detection config"))?;
            let row = match updated {
                Some(row) => row,
                None => sqlx::query_as::<_, ConfigRow>(
                    "insert into intelligence_detection_configs
                     (enabled, account_scope, interval_secs, model)
                     values ($1, $2, $3, $4)
                     returning enabled, account_scope, interval_secs, model, updated_at",
                )
                .bind(command.enabled)
                .bind(&scope)
                .bind(interval_secs)
                .bind(&command.model)
                .fetch_one(&mut *transaction)
                .await
                .map_err(|_| postgres_unavailable("insert detection config"))?,
            };
            append_admin_audit_event_in_transaction(&mut transaction, audit, revision).await?;
            Ok((revision, detection_config_from_row(row)?))
        }
        .await;
        match result {
            Ok(value) => {
                transaction
                    .commit()
                    .await
                    .map_err(|_| postgres_unavailable("commit detection config replace"))?;
                Ok(value)
            }
            Err(error) => {
                transaction
                    .rollback()
                    .await
                    .map_err(|_| postgres_unavailable("rollback detection config replace"))?;
                Err(error)
            }
        }
    }

    async fn load_records(&self, query: &DetectionRecordQuery) -> StoreResult<Vec<RecordRow>> {
        let limit = i64::from(query.page_size);
        let offset = (i64::from(query.page) - 1) * i64::from(query.page_size);
        let rows = match query.detection_round_id {
            Some(round_id) => {
                sqlx::query_as::<_, RecordRow>(
                    "select r.id, r.detection_round_id::text, r.account_id,
                            a.email, a.name, a.provider_kind, a.plan_type,
                            a.scheduling_suspended, r.checked_at, r.degraded
                     from intelligence_detection_records r
                     join provider_accounts a on a.id = r.account_id
                     where r.detection_round_id = $1::uuid
                     order by r.checked_at desc, r.id desc limit $2 offset $3",
                )
                .bind(round_id.to_string())
                .bind(limit)
                .bind(offset)
                .fetch_all(&self.pool)
                .await
            }
            None => {
                sqlx::query_as::<_, RecordRow>(
                    "select r.id, r.detection_round_id::text, r.account_id,
                            a.email, a.name, a.provider_kind, a.plan_type,
                            a.scheduling_suspended, r.checked_at, r.degraded
                     from intelligence_detection_records r
                     join provider_accounts a on a.id = r.account_id
                     order by r.checked_at desc, r.id desc limit $1 offset $2",
                )
                .bind(limit)
                .bind(offset)
                .fetch_all(&self.pool)
                .await
            }
        };
        rows.map_err(|_| postgres_unavailable("list detection records"))
    }

    async fn load_rounds(&self, limit: u32) -> StoreResult<Vec<RoundRow>> {
        sqlx::query_as::<_, RoundRow>(
            "select detection_round_id::text,
                    max(checked_at),
                    count(*) filter (where degraded),
                    count(*) filter (where not degraded)
             from intelligence_detection_records
             group by detection_round_id
             order by max(checked_at) desc
             limit $1",
        )
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(|_| postgres_unavailable("list detection rounds"))
    }

    async fn load_targets(&self, scope: &DetectionAccountScope) -> StoreResult<Vec<TargetRow>> {
        let rows = match scope {
            DetectionAccountScope::AllAccounts => {
                sqlx::query_as::<_, TargetRow>(
                    "select id, provider_kind, scheduling_suspended, scheduling_suspended_by
                     from provider_accounts order by id",
                )
                .fetch_all(&self.pool)
                .await
            }
            DetectionAccountScope::SelectedAccounts { account_ids } => {
                sqlx::query_as::<_, TargetRow>(
                    "select id, provider_kind, scheduling_suspended, scheduling_suspended_by
                     from provider_accounts where id = any($1::text[]) order by id",
                )
                .bind(account_ids.as_slice())
                .fetch_all(&self.pool)
                .await
            }
        };
        rows.map_err(|_| postgres_unavailable("list detection targets"))
    }

    async fn load_record_html(&self, record_id: i64) -> StoreResult<Option<String>> {
        let row = sqlx::query_scalar::<_, Option<String>>(
            "select html_content from intelligence_detection_records where id = $1",
        )
        .bind(record_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| postgres_unavailable("load detection record html"))?;
        Ok(row.flatten())
    }

    async fn insert_record(&self, record: &NewDetectionRecord) -> StoreResult<()> {
        sqlx::query(
            "insert into intelligence_detection_records
             (detection_round_id, account_id, degraded, html_content, matched_phrases)
             values ($1::uuid, $2, $3, $4, $5)",
        )
        .bind(record.detection_round_id.to_string())
        .bind(&record.account_id)
        .bind(record.degraded)
        .bind(record.html_content.as_deref())
        .bind(record.matched_phrases.as_slice())
        .execute(&self.pool)
        .await
        .map_err(|_| postgres_unavailable("insert detection record"))?;
        Ok(())
    }
}

#[async_trait]
impl DetectionStore for PgDetectionStore {
    async fn load_detection_config(&self) -> AdminStoreResult<Option<DetectionConfig>> {
        self.load_config()
            .await
            .map_err(|error| admin_store_error(ENTITY, error))
    }

    async fn replace_detection_config(
        &self,
        command: ReplaceDetectionConfig,
        context: &MutationContext,
    ) -> AdminStoreResult<DetectionConfigMutation> {
        let audit = mutation_audit(
            context,
            "detection_config.replace",
            "intelligence_detection_config",
            "1",
            ["enabled", "account_scope", "interval_secs", "model"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
        );
        let (revision, config) = self
            .replace_config(command, audit)
            .await
            .map_err(|error| admin_store_error(ENTITY, error))?;
        Ok(DetectionConfigMutation {
            config_revision: admin_revision(revision)?,
            config,
        })
    }

    async fn list_detection_records(
        &self,
        query: DetectionRecordQuery,
    ) -> AdminStoreResult<Vec<DetectionRecord>> {
        self.load_records(&query)
            .await
            .and_then(|rows| rows.into_iter().map(detection_record_from_row).collect())
            .map_err(|error| admin_store_error(ENTITY, error))
    }

    async fn list_detection_rounds(&self, limit: u32) -> AdminStoreResult<Vec<DetectionRound>> {
        self.load_rounds(limit)
            .await
            .and_then(|rows| rows.into_iter().map(detection_round_from_row).collect())
            .map_err(|error| admin_store_error(ENTITY, error))
    }

    async fn list_detection_targets(
        &self,
        scope: &DetectionAccountScope,
    ) -> AdminStoreResult<Vec<DetectionTarget>> {
        self.load_targets(scope)
            .await
            .and_then(|rows| rows.into_iter().map(detection_target_from_row).collect())
            .map_err(|error| admin_store_error(ENTITY, error))
    }

    async fn insert_detection_record(&self, record: NewDetectionRecord) -> AdminStoreResult<()> {
        self.insert_record(&record)
            .await
            .map_err(|error| admin_store_error(ENTITY, error))
    }

    async fn load_detection_record_html(&self, record_id: i64) -> AdminStoreResult<Option<String>> {
        self.load_record_html(record_id)
            .await
            .map_err(|error| admin_store_error(ENTITY, error))
    }
}

fn detection_config_from_row(row: ConfigRow) -> StoreResult<DetectionConfig> {
    let (enabled, scope, interval_secs, model, updated_at) = row;
    Ok(DetectionConfig {
        enabled,
        account_scope: parse_account_scope(&scope)?,
        interval_secs: u32::try_from(interval_secs)
            .map_err(|_| invalid("negative detection interval"))?,
        model,
        updated_at,
    })
}

fn detection_record_from_row(row: RecordRow) -> StoreResult<DetectionRecord> {
    let (
        id,
        detection_round_id,
        account_id,
        account_email,
        account_name,
        account_provider_kind,
        account_plan_type,
        scheduling_suspended,
        checked_at,
        degraded,
    ) = row;
    Ok(DetectionRecord {
        id,
        detection_round_id: parse_round_id(&detection_round_id)?,
        account_id,
        account_email,
        account_name: Some(account_name),
        account_provider_kind,
        account_plan_type,
        account_plan_type_display: None,
        checked_at,
        degraded,
        scheduling_suspended,
    })
}

fn detection_round_from_row(row: RoundRow) -> StoreResult<DetectionRound> {
    let (detection_round_id, checked_at, degraded_count, normal_count) = row;
    Ok(DetectionRound {
        detection_round_id: parse_round_id(&detection_round_id)?,
        checked_at,
        degraded_count: u64::try_from(degraded_count)
            .map_err(|_| invalid("negative degraded count"))?,
        normal_count: u64::try_from(normal_count).map_err(|_| invalid("negative normal count"))?,
    })
}

fn detection_target_from_row(row: TargetRow) -> StoreResult<DetectionTarget> {
    let (account_id, provider_kind, scheduling_suspended, scheduling_suspended_by) = row;
    let scheduling_suspended_by = scheduling_suspended_by
        .as_deref()
        .map(|value| {
            SchedulingSuspensionSource::parse(value)
                .ok_or_else(|| invalid("unknown scheduling_suspended_by value"))
        })
        .transpose()?;
    Ok(DetectionTarget {
        account_id,
        provider_kind: ProviderKind::new(provider_kind)
            .map_err(|_| invalid("invalid provider kind"))?,
        scheduling_suspended,
        scheduling_suspended_by,
    })
}

fn parse_round_id(value: &str) -> StoreResult<Uuid> {
    Uuid::parse_str(value).map_err(|_| invalid("invalid detection round id"))
}

/// 解析 `account_scope` JSONB：`{"all": true}` 或 `{"account_ids": [...]}`。
fn parse_account_scope(value: &serde_json::Value) -> StoreResult<DetectionAccountScope> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid("detection account scope is not an object"))?;
    if object.get("all").and_then(serde_json::Value::as_bool) == Some(true) {
        return Ok(DetectionAccountScope::AllAccounts);
    }
    if let Some(ids) = object
        .get("account_ids")
        .and_then(serde_json::Value::as_array)
    {
        let account_ids = ids
            .iter()
            .map(|id| {
                id.as_str()
                    .map(ToOwned::to_owned)
                    .ok_or_else(|| invalid("detection account id is not a string"))
            })
            .collect::<StoreResult<Vec<_>>>()?;
        return Ok(DetectionAccountScope::SelectedAccounts { account_ids });
    }
    Err(invalid("unknown detection account scope"))
}

fn account_scope_json(scope: &DetectionAccountScope) -> serde_json::Value {
    match scope {
        DetectionAccountScope::AllAccounts => serde_json::json!({ "all": true }),
        DetectionAccountScope::SelectedAccounts { account_ids } => {
            serde_json::json!({ "account_ids": account_ids })
        }
    }
}

fn invalid(message: &str) -> StoreError {
    StoreError::InvalidData {
        entity: ENTITY,
        message: message.to_owned(),
    }
}
