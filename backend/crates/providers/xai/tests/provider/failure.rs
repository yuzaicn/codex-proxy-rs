use gateway_core::error::{ProviderError, ProviderErrorKind};
use gateway_core::upstream::UpstreamSendState;
use provider_xai::xai_failure_affects_account_score;

#[test]
fn account_score_filter_preserves_confirmed_failure_semantics() {
    for (send_state, expected) in [
        (UpstreamSendState::Sent, true),
        (UpstreamSendState::Ambiguous, false),
        (UpstreamSendState::NotSent, false),
    ] {
        let error = ProviderError::new(ProviderErrorKind::Transport, send_state);
        assert_eq!(xai_failure_affects_account_score(&error), expected);
    }
}
