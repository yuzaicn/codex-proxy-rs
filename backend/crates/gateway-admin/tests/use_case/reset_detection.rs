use gateway_admin::model::{
    AdminErrorKind, MutationContext,
    reset_detection::{ReplaceResetDetectionSettings, ResetDetectionAccountScope},
};

fn command(poll_interval_secs: u32) -> ReplaceResetDetectionSettings {
    ReplaceResetDetectionSettings {
        enabled: false,
        poll_interval_secs,
        account_scope: ResetDetectionAccountScope::AllNonError,
        auto_consume_enabled: false,
    }
}

fn context() -> MutationContext {
    MutationContext {
        actor: gateway_admin::model::MutationActor::System,
        request_id: "request-reset-detection".to_owned(),
    }
}

#[tokio::test]
async fn replace_should_reject_poll_interval_29_before_store_call() {
    let services = super::AdminHarness::new().build().await;
    let error = services
        .reset_detection()
        .replace(&context(), command(29))
        .await
        .expect_err("poll interval below the frozen 30s minimum");
    assert_eq!(error.kind(), AdminErrorKind::Invalid);
}

#[tokio::test]
async fn replace_should_accept_poll_interval_30_and_reach_the_store() {
    let services = super::AdminHarness::new().build().await;
    let error = services
        .reset_detection()
        .replace(&context(), command(30))
        .await
        .expect_err("harness reset detection store is unavailable");
    assert_ne!(error.kind(), AdminErrorKind::Invalid);
}
