use gateway_admin::{
    model::detection::{DetectionAccountScope, NewDetectionRecord},
    ports::store::DetectionStore,
};
use gateway_store::postgres::PgDetectionStore;
use uuid::Uuid;

use super::TestDatabase;

#[tokio::test]
async fn mark_suspension_released_updates_round_recovered_count() {
    let Some(database) = TestDatabase::create("detection_recovery_count").await else {
        return;
    };
    sqlx::query(
        "insert into provider_accounts (
           id, provider_kind, name, authentication_kind, provider_credentials_json,
           credential_revision, has_refresh_token, enabled, credential_state,
           credential_observed_at, created_at, updated_at
         ) values ($1, 'openai', 'detection test', 'oauth', '{}'::jsonb,
                   1, true, true, 'ready', now(), now(), now())",
    )
    .bind("acct_detection_recovery")
    .execute(&database.pool)
    .await
    .expect("seed provider account");

    let round_id = Uuid::new_v4();
    let store = PgDetectionStore::new(database.pool.clone());
    store
        .insert_detection_record(NewDetectionRecord {
            detection_round_id: round_id,
            account_id: "acct_detection_recovery".to_owned(),
            degraded: false,
            html_content: None,
            reasoning_content: None,
            prompt_used: None,
            matched_phrases: Vec::new(),
            suspension_released: false,
        })
        .await
        .expect("insert detection record");

    let before = store
        .list_detection_rounds(10)
        .await
        .expect("load detection rounds");
    assert_eq!(before[0].recovered_count, Some(0));

    store
        .mark_suspension_released(round_id, "acct_detection_recovery")
        .await
        .expect("mark suspension released");
    let after = store
        .list_detection_rounds(10)
        .await
        .expect("reload detection rounds");
    assert_eq!(after[0].recovered_count, Some(1));
    database.close().await;
}

#[tokio::test]
async fn detection_targets_exclude_non_openai_accounts_for_all_scopes() {
    let Some(database) = TestDatabase::create("detection_openai_targets").await else {
        return;
    };
    for (id, provider) in [("acct_openai", "openai"), ("acct_xai", "xai")] {
        sqlx::query(
            "insert into provider_accounts (
               id, provider_kind, name, authentication_kind, provider_credentials_json,
               credential_revision, has_refresh_token, enabled, credential_state,
               credential_observed_at, created_at, updated_at
             ) values ($1, $2, 'test', 'oauth', '{}'::jsonb, 1, true, true, 'ready', now(), now(), now())",
        )
        .bind(id)
        .bind(provider)
        .execute(&database.pool)
        .await
        .expect("seed provider account");
    }
    let store = PgDetectionStore::new(database.pool.clone());
    let all = store
        .list_detection_targets(&DetectionAccountScope::AllAccounts)
        .await
        .expect("load all detection targets");
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].account_id, "acct_openai");

    let selected = store
        .list_detection_targets(&DetectionAccountScope::SelectedAccounts {
            account_ids: vec!["acct_openai".to_owned(), "acct_xai".to_owned()],
        })
        .await
        .expect("load selected detection targets");
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].account_id, "acct_openai");
    database.close().await;
}
