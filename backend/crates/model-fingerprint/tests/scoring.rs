use model_fingerprint::{
    FingerprintBank, FingerprintScorer, UPSTREAM_ORDERED_BLOCK_WEIGHT, parse_numbers,
};
use serde::Deserialize;

const TOLERANCE: f64 = 1e-9;
const GOLDEN: &[u8] = include_bytes!("fixtures/modeltrace-golden-20260912.json");

#[derive(Deserialize)]
struct GoldenFile {
    source_commit: String,
    bank_sha256: String,
    model_order: Vec<String>,
    cases: Vec<GoldenCase>,
}

#[derive(Deserialize)]
struct GoldenCase {
    id: String,
    claimed_model: String,
    text: String,
    weights: Vec<GoldenScore>,
}

#[derive(Deserialize)]
struct GoldenScore {
    ordered_block_weight: f64,
    scores: Vec<f64>,
    rank_one: String,
    margin: f64,
}

#[test]
fn parser_preserves_upstream_run_boundaries() {
    assert_eq!(parse_numbers("1 2 中文 4 5 6"), vec![4, 5, 6]);
    assert_eq!(parse_numbers("1 2 3 English 4 5 6"), vec![1, 2, 3]);
    assert_eq!(parse_numbers("1 2 1418 3"), vec![1, 2, 3]);
}

#[test]
fn all_reference_responses_match_both_weight_paths() {
    let golden: GoldenFile = serde_json::from_slice(GOLDEN).expect("golden JSON");
    assert_eq!(
        golden.source_commit,
        "bff6cb1ff6f91e70a4e61d3f106f8c4925918eea"
    );
    assert_eq!(
        golden.bank_sha256,
        "5886b3afc6302abee7fe3637a6578c39b9f6d2932cbc9029f6c1c4c6f398bca6"
    );
    assert_eq!(golden.cases.len(), 468);

    let marginal = FingerprintScorer::new(FingerprintBank::embedded().expect("embedded bank"));
    let upstream = FingerprintScorer::with_ordered_block_weight(
        FingerprintBank::embedded().expect("embedded bank"),
        UPSTREAM_ORDERED_BLOCK_WEIGHT,
    )
    .expect("upstream weight");
    assert_eq!(marginal.model_order(), golden.model_order);
    assert_eq!(upstream.model_order(), golden.model_order);

    for case in golden.cases {
        assert_eq!(case.weights.len(), 2, "{} weight paths", case.id);
        for expected in case.weights {
            let scorer = if expected.ordered_block_weight == 0.0 {
                &marginal
            } else {
                assert_eq!(
                    expected.ordered_block_weight, UPSTREAM_ORDERED_BLOCK_WEIGHT,
                    "{} unexpected weight",
                    case.id
                );
                &upstream
            };
            let actual = scorer
                .score(&case.text, &case.claimed_model)
                .unwrap_or_else(|error| panic!("{} scoring failed: {error}", case.id));
            assert_eq!(actual.scores.len(), 33, "{} score dimensions", case.id);
            assert_eq!(actual.rank_one, expected.rank_one, "{} rank-1", case.id);
            assert_close(actual.margin, expected.margin, &case.id, "margin", None);
            for (index, (actual, expected)) in
                actual.scores.iter().zip(&expected.scores).enumerate()
            {
                assert_close(*actual, *expected, &case.id, "score", Some(index));
            }
        }
    }
}

fn assert_close(actual: f64, expected: f64, case: &str, field: &str, index: Option<usize>) {
    let difference = (actual - expected).abs();
    assert!(
        difference <= TOLERANCE,
        "{case} {field} {index:?}: actual={actual:.17}, expected={expected:.17}, diff={difference:.3e}"
    );
}
