use gateway_admin::model::{
    MutationActor, MutationContext,
    reset_detection::{ReplaceResetDetectionSettings, ResetDetectionAccountScope},
};
use gateway_admin::ports::store::ResetDetectionStore;
use gateway_store::postgres::PgResetDetectionStore;

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
