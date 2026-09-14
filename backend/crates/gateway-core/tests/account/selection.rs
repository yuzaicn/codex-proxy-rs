use std::time::{Duration, Instant};

use gateway_core::account::{
    AccountAttemptFeedback, AccountCandidate, AccountFeedbackStats, AccountSelector,
    ProviderAccountId, RotationStrategy,
};
use gateway_core::routing::ProviderKind;

use super::{candidate, candidate_with_concurrency, context};

const FAILURE_RATE_HALF_LIFE: Duration = Duration::from_secs(15 * 60);

fn feedback_subject() -> (AccountFeedbackStats, ProviderKind, ProviderAccountId) {
    (
        AccountFeedbackStats::default(),
        ProviderKind::new("openai").expect("valid provider"),
        ProviderAccountId::new("acct_decay").expect("valid account"),
    )
}

fn report_failure(
    feedback: &AccountFeedbackStats,
    provider: &ProviderKind,
    account: &ProviderAccountId,
    observed_at: Instant,
) {
    feedback.report_at(
        provider,
        account,
        AccountAttemptFeedback::Failed {
            first_output_ms: None,
        },
        observed_at,
    );
}

#[test]
fn account_failure_rate_should_halve_after_one_half_life() {
    let (feedback, provider, account) = feedback_subject();
    let observed_at = Instant::now();
    report_failure(&feedback, &provider, &account, observed_at);

    let failure_rate = feedback
        .scheduling_signals_at(&provider, &account, observed_at + FAILURE_RATE_HALF_LIFE)
        .0;

    assert_eq!(failure_rate, Some(1_000));
}

#[test]
fn account_failure_rate_should_quarter_after_two_half_lives() {
    let (feedback, provider, account) = feedback_subject();
    let observed_at = Instant::now();
    report_failure(&feedback, &provider, &account, observed_at);

    let failure_rate = feedback
        .scheduling_signals_at(
            &provider,
            &account,
            observed_at + FAILURE_RATE_HALF_LIFE * 2,
        )
        .0;

    assert_eq!(failure_rate, Some(500));
}

#[test]
fn account_failure_rate_should_decay_before_applying_a_new_sample() {
    let (feedback, provider, account) = feedback_subject();
    let observed_at = Instant::now();
    report_failure(&feedback, &provider, &account, observed_at);
    feedback.report_at(
        &provider,
        &account,
        AccountAttemptFeedback::Succeeded {
            first_output_ms: None,
        },
        observed_at + FAILURE_RATE_HALF_LIFE,
    );

    let failure_rate = feedback
        .scheduling_signals_at(&provider, &account, observed_at + FAILURE_RATE_HALF_LIFE)
        .0;

    assert_eq!(failure_rate, Some(800));
}

#[test]
fn concurrent_account_failures_should_not_lose_samples() {
    let (feedback, provider, account) = feedback_subject();
    let observed_at = Instant::now();
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| report_failure(&feedback, &provider, &account, observed_at));
        }
    });

    let failure_rate = feedback
        .scheduling_signals_at(&provider, &account, observed_at)
        .0;

    assert_eq!(failure_rate, Some(8_322));
}
fn smart_selection_ids(candidates: &[AccountCandidate]) -> Vec<&str> {
    let mut selection = context(RotationStrategy::Smart);
    (0..20)
        .map(|cursor| {
            selection.round_robin_cursor = cursor;
            AccountSelector
                .select(candidates, &selection)
                .expect("candidate available")
                .candidate()
                .account
                .id()
                .as_str()
        })
        .collect()
}

#[test]
fn smart_selector_should_rotate_despite_small_signal_differences() {
    for signal in ["quota", "latency", "load", "failure"] {
        let mut candidates = [
            candidate_with_concurrency("acct_a", 0, 100),
            candidate_with_concurrency("acct_b", 0, 100),
        ];
        match signal {
            "quota" => {
                candidates[0].signals.quota_remaining_rank = Some(600);
                candidates[1].signals.quota_remaining_rank = Some(601);
            }
            "latency" => {
                candidates[0].signals.first_output_latency_ms = Some(2_500);
                candidates[1].signals.first_output_latency_ms = Some(2_501);
            }
            "load" => candidates[0].signals.in_flight = 1,
            "failure" => candidates[0].signals.failure_rate_basis_points = Some(1),
            _ => unreachable!(),
        }
        let selected = smart_selection_ids(&candidates);

        assert_eq!(selected, ["acct_a", "acct_b"].repeat(10), "{signal}");
    }
}

#[test]
fn smart_selector_should_keep_rotation_order_when_nearby_scores_cross() {
    let mut candidates = [
        candidate("acct_a", 0, Some(8_000)),
        candidate("acct_b", 0, Some(8_001)),
        candidate("acct_worse", 0, Some(2_000)),
    ];
    let mut selection = context(RotationStrategy::Smart);
    let mut selected = Vec::new();
    for cursor in 0..20 {
        selection.round_robin_cursor = cursor;
        for candidate in &mut candidates {
            match candidate.account.id().as_str() {
                "acct_a" => candidate.signals.quota_remaining_rank = Some(8_000 + cursor % 2),
                "acct_b" => candidate.signals.quota_remaining_rank = Some(8_001 - cursor % 2),
                _ => {}
            }
        }
        candidates.reverse();
        selected.push(
            AccountSelector
                .select(&candidates, &selection)
                .expect("candidate available")
                .candidate()
                .account
                .id()
                .as_str()
                .to_owned(),
        );
    }

    assert_eq!(selected, ["acct_a", "acct_b"].repeat(10));
}

#[test]
fn smart_selector_should_only_rotate_among_candidates_close_to_the_best() {
    let mut candidates = [
        candidate("acct_a", 0, Some(10_000)),
        candidate("acct_b", 0, Some(9_500)),
        candidate("acct_c", 0, Some(9_000)),
    ];
    for candidate in &mut candidates {
        candidate.signals.failure_rate_basis_points = Some(0);
    }

    assert_eq!(
        smart_selection_ids(&candidates),
        ["acct_a", "acct_b"].repeat(10)
    );
}

#[test]
fn smart_selector_should_preserve_material_signal_advantages() {
    for signal in ["quota", "latency", "load", "failure"] {
        let mut candidates = [
            candidate_with_concurrency("acct_a", 0, 10),
            candidate_with_concurrency("acct_b", 0, 10),
        ];
        match signal {
            "quota" => {
                candidates[0].signals.quota_remaining_rank = Some(600);
                candidates[1].signals.quota_remaining_rank = Some(2_600);
            }
            "latency" => {
                candidates[0].signals.first_output_latency_ms = Some(20_000);
                candidates[1].signals.first_output_latency_ms = Some(2_500);
            }
            "load" => candidates[0].signals.in_flight = 1,
            "failure" => candidates[0].signals.failure_rate_basis_points = Some(2_000),
            _ => unreachable!(),
        }

        assert_eq!(smart_selection_ids(&candidates), ["acct_b"; 20], "{signal}");
    }
}

#[test]
fn smart_selector_should_balance_actual_load_against_remaining_quota() {
    let mut candidates = [
        candidate_with_concurrency("acct_a", 0, 10),
        candidate_with_concurrency("acct_b", 1, 10),
    ];
    candidates[0].signals.quota_remaining_rank = Some(600);
    candidates[1].signals.quota_remaining_rank = Some(2_600);

    assert_eq!(smart_selection_ids(&candidates), ["acct_b"; 20]);
}

#[test]
fn smart_selector_should_treat_unknown_quota_as_neutral() {
    let candidates = [
        candidate("acct_known_low", 0, Some(600)),
        candidate("acct_unknown", 0, None),
    ];

    assert_eq!(smart_selection_ids(&candidates), ["acct_unknown"; 20]);
}

#[test]
fn smart_selector_should_treat_unknown_latency_as_neutral() {
    let mut unknown = candidate("acct_unknown", 0, Some(5_000));
    unknown.signals.failure_rate_basis_points = Some(0);
    let mut proven_fast = candidate("acct_proven_fast", 0, Some(5_000));
    proven_fast.signals.failure_rate_basis_points = Some(0);
    proven_fast.signals.first_output_latency_ms = Some(100);

    assert_eq!(
        smart_selection_ids(&[unknown, proven_fast]),
        ["acct_proven_fast"; 20]
    );
}

#[test]
fn smart_selector_should_cap_zero_feedback_quota_until_any_feedback_arrives() {
    let fresh = candidate("acct_fresh", 0, Some(10_000));
    let mut observed = candidate("acct_observed", 0, Some(5_000));
    observed.signals.failure_rate_basis_points = Some(0);

    assert_eq!(
        smart_selection_ids(&[fresh.clone(), observed]),
        ["acct_fresh", "acct_observed"].repeat(10)
    );

    for signal in ["failure", "latency"] {
        let mut warmed = fresh.clone();
        match signal {
            "failure" => warmed.signals.failure_rate_basis_points = Some(0),
            "latency" => warmed.signals.first_output_latency_ms = Some(10_000),
            _ => unreachable!(),
        }
        let cold = candidate("acct_cold", 0, Some(5_000));
        assert_eq!(smart_selection_ids(&[warmed, cold]), ["acct_fresh"; 20]);
    }
}

#[test]
fn smart_selector_should_demote_an_account_after_repeated_terminal_failures() {
    let feedback = AccountFeedbackStats::default();
    let provider = ProviderKind::new("openai").expect("provider");
    let rejected_id = ProviderAccountId::new("acct_rejected").expect("account");
    for _ in 0..3 {
        feedback.report(
            &provider,
            &rejected_id,
            AccountAttemptFeedback::Failed {
                first_output_ms: None,
            },
        );
    }
    let (failure_rate, latency) = feedback.scheduling_signals(&provider, &rejected_id);
    assert!(failure_rate.is_some_and(|rate| rate > 0));

    let mut rejected = candidate("acct_rejected", 0, Some(10_000));
    rejected.signals.failure_rate_basis_points = failure_rate;
    rejected.signals.first_output_latency_ms = latency;
    let mut healthy = candidate("acct_healthy", 0, Some(5_000));
    healthy.signals.failure_rate_basis_points = Some(0);
    healthy.signals.first_output_latency_ms = Some(100);

    assert_eq!(
        smart_selection_ids(&[rejected, healthy]),
        ["acct_healthy"; 20]
    );
}
