use gateway_admin::model::{
    MutationActor, MutationContext,
    reset_detection::{
        CompleteResetCreditConsume, ReplaceResetDetectionSettings, ResetCreditConsumeOutcome,
        ResetDetectionAccountScope,
    },
};
use gateway_admin::ports::store::ResetDetectionStore;
use gateway_core::account::{AccountStatus, ProviderAccountId};
use gateway_store::postgres::PgResetDetectionStore;
use uuid::Uuid;

use super::TestDatabase;

fn context() -> MutationContext {
    MutationContext {
        actor: MutationActor::System,
        request_id: "reset-detection-store-test".to_owned(),
    }
}

#[tokio::test]
async fn reset_detection_settings_should_load_and_replace_in_postgres() {
    let Some(database) = TestDatabase::create("reset_detection_settings").await else {
        return;
    };
    let store = PgResetDetectionStore::new(database.pool.clone());
    let before = store
        .load_reset_detection_settings()
        .await
        .expect("load initial settings");
    assert_eq!(before.poll_interval_secs, 3600);
    let mutation = store
        .replace_reset_detection_settings(
            ReplaceResetDetectionSettings {
                enabled: true,
                poll_interval_secs: 7200,
                account_scope: ResetDetectionAccountScope::Limited,
                auto_consume_enabled: true,
            },
            &context(),
        )
        .await
        .expect("replace settings");
    assert!(mutation.config_revision.get() > before.config_revision.get());
    assert_eq!(mutation.settings.poll_interval_secs, 7200);
    assert_eq!(
        mutation.settings.account_scope,
        ResetDetectionAccountScope::Limited
    );
    assert!(mutation.settings.auto_consume_enabled);
    let after = store
        .load_reset_detection_settings()
        .await
        .expect("reload settings");
    assert_eq!(after, mutation.settings);
    database.close().await;
}

#[tokio::test]
async fn reset_credit_observation_and_consume_reservation_should_persist() {
    let Some(database) = TestDatabase::create("reset_credit_worker_state").await else {
        return;
    };
    sqlx::query(
        "insert into provider_accounts (
           id, provider_kind, name, authentication_kind, provider_credentials_json,
           credential_revision, has_refresh_token, enabled, credential_state,
           credential_observed_at, created_at, updated_at
         ) values ($1, 'openai', 'test', 'oauth', '{}'::jsonb, 1, true, true, 'ready', now(), now(), now())",
    )
    .bind("acct_reset_worker")
    .execute(&database.pool)
    .await
    .expect("seed provider account");
    let store = PgResetDetectionStore::new(database.pool.clone());
    let account_id = ProviderAccountId::new("acct_reset_worker").expect("account ID");
    let observed_at = chrono::Utc::now();
    store
        .record_reset_credits_observation(&account_id, 2, observed_at)
        .await
        .expect("record reset credits observation");
    let observation = sqlx::query_as::<_, (i64, chrono::DateTime<chrono::Utc>)>(
        "select reset_credits_available_count, reset_credits_observed_at
           from provider_accounts where id = $1",
    )
    .bind(account_id.as_str())
    .fetch_one(&database.pool)
    .await
    .expect("load reset credits observation");
    assert_eq!(observation.0, 2);
    assert_eq!(
        observation.1.timestamp_micros(),
        observed_at.timestamp_micros()
    );

    let first_id = Uuid::new_v4();
    let second_id = Uuid::new_v4();
    let first = store
        .reserve_reset_credit_consume(&account_id, first_id)
        .await
        .expect("reserve first consume");
    assert_eq!(first.redeem_request_id, first_id);
    assert!(!first.resumed);
    let resumed = store
        .reserve_reset_credit_consume(&account_id, second_id)
        .await
        .expect("resume pending consume");
    assert_eq!(resumed.redeem_request_id, first_id);
    assert!(resumed.resumed);

    store
        .complete_reset_credit_consume(CompleteResetCreditConsume {
            account_id: account_id.clone(),
            redeem_request_id: first_id,
            outcome: ResetCreditConsumeOutcome::Confirmed {
                provider_code: "already_redeemed".to_owned(),
                quota_status: AccountStatus::Normal,
            },
        })
        .await
        .expect("complete consume");
    let next = store
        .reserve_reset_credit_consume(&account_id, second_id)
        .await
        .expect("reserve next consume");
    assert_eq!(next.redeem_request_id, second_id);
    assert!(!next.resumed);

    database.close().await;
}
