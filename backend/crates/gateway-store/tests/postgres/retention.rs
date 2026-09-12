use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use gateway_store::postgres::{
    PgRetentionRepository, RetentionCycleBudget, RetentionRepository as _, RuntimeRetentionSettings,
};

use super::TestDatabase;

#[test]
fn retention_settings_preserve_independent_windows() {
    let settings = RuntimeRetentionSettings {
        usage_retention_days: 31,
        ops_event_retention_days: 30,
        audit_retention_days: 90,
    };
    assert_eq!(settings.audit_retention_days, 90);
}

#[tokio::test]
async fn retention_cycle_should_stop_at_the_batch_budget() {
    let Some(database) = TestDatabase::create("retention_cycle_budget").await else {
        return;
    };
    let expired_at = Utc::now() - ChronoDuration::days(100);
    sqlx::query(
        "insert into admin_audit_events (
           id, actor_kind, actor_ref, action, entity_kind, entity_ref,
           changed_fields, created_at
         )
         select 'retention-' || value::text, 'system', 'retention', 'cleanup',
                'fixture', 'fixture-' || value::text, array[]::text[], $1
         from generate_series(1, 5) as value",
    )
    .bind(expired_at)
    .execute(&database.pool)
    .await
    .expect("seed expired audit events");

    let budget = RetentionCycleBudget::try_new(2, 3, Duration::from_secs(1), Duration::ZERO)
        .expect("retention cycle budget");
    let repository = PgRetentionRepository::with_cycle_budget(database.pool.clone(), budget);
    let report = repository
        .apply_retention(
            Utc::now(),
            RuntimeRetentionSettings {
                usage_retention_days: 31,
                ops_event_retention_days: 30,
                audit_retention_days: 90,
            },
        )
        .await
        .expect("bounded retention cycle");

    assert_eq!(report.model_requests, 0);
    assert_eq!(report.ops_events, 0);
    assert_eq!(report.admin_audit_events, 2);
    assert_eq!(report.batches, 3);
    assert!(report.budget_exhausted);
    let remaining: i64 =
        sqlx::query_scalar("select count(*) from admin_audit_events where id like 'retention-%'")
            .fetch_one(&database.pool)
            .await
            .expect("count remaining audit events");
    assert_eq!(remaining, 3);
    database.close().await;
}

#[tokio::test]
async fn retention_preserves_only_unfinished_reset_consume_reservations() {
    let Some(database) = TestDatabase::create("retention_reset_consume").await else {
        return;
    };
    let expired_at = Utc::now() - ChronoDuration::days(100);
    sqlx::query(
        "insert into admin_audit_events (
           id, actor_kind, actor_ref, admin_request_id, action, entity_kind, entity_ref,
           changed_fields, created_at
         ) values
           ('pending-start', 'system', 'system', '11111111-1111-4111-8111-111111111111',
            'reset_detection.auto_consume_started', 'provider_account', 'acct_pending', '{}', $1),
           ('done-start', 'system', 'system', '22222222-2222-4222-8222-222222222222',
            'reset_detection.auto_consume_started', 'provider_account', 'acct_done', '{}', $1),
           ('done-finish', 'system', 'system', '22222222-2222-4222-8222-222222222222',
            'reset_detection.auto_consume_finished', 'provider_account', 'acct_done', '{}', $1)",
    )
    .bind(expired_at)
    .execute(&database.pool)
    .await
    .expect("seed reset consume audit events");

    let budget = RetentionCycleBudget::try_new(1, 10, Duration::from_secs(1), Duration::ZERO)
        .expect("retention cycle budget");
    let repository = PgRetentionRepository::with_cycle_budget(database.pool.clone(), budget);
    repository
        .apply_retention(
            Utc::now(),
            RuntimeRetentionSettings {
                usage_retention_days: 31,
                ops_event_retention_days: 30,
                audit_retention_days: 90,
            },
        )
        .await
        .expect("retention cycle");

    let remaining = sqlx::query_as::<_, (String, String)>(
        "select id, action from admin_audit_events order by id",
    )
    .fetch_all(&database.pool)
    .await
    .expect("load retained audit events");
    assert_eq!(
        remaining,
        vec![(
            "pending-start".to_owned(),
            "reset_detection.auto_consume_started".to_owned(),
        )]
    );
    database.close().await;
}
