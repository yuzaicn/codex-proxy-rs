use model_fingerprint::{
    DEFAULT_ORDERED_BLOCK_WEIGHT, FingerprintBank, FingerprintError, FingerprintScorer,
    UPSTREAM_ORDERED_BLOCK_WEIGHT,
};

const BANK: &[u8] = include_bytes!("../assets/modeltrace-bank-20260912.json");

#[test]
fn embedded_bank_exposes_pinned_metadata_and_default_weight() {
    let bank = FingerprintBank::embedded().expect("embedded bank");
    assert_eq!(bank.built_at(), "2026-09-12T07:16:18.673776+00:00");
    assert_eq!(bank.model_order().len(), 33);
    assert_eq!(
        bank.upstream_ordered_block_weight(),
        UPSTREAM_ORDERED_BLOCK_WEIGHT
    );

    let scorer = FingerprintScorer::new(bank);
    assert_eq!(scorer.ordered_block_weight(), DEFAULT_ORDERED_BLOCK_WEIGHT);
}

#[test]
fn bank_rejects_invalid_json_missing_fields_and_wrong_dimensions() {
    assert!(matches!(
        FingerprintBank::from_json(b"not json"),
        Err(FingerprintError::InvalidBankJson(_))
    ));

    let mut missing: serde_json::Value = serde_json::from_slice(BANK).expect("bank JSON");
    missing["robust"]["hellinger"]
        .as_object_mut()
        .expect("hellinger object")
        .remove("feature_mean");
    let missing = serde_json::to_vec(&missing).expect("serialize missing field bank");
    assert!(matches!(
        FingerprintBank::from_json(&missing),
        Err(FingerprintError::InvalidBankJson(_))
    ));

    let mut wrong_dimension: serde_json::Value = serde_json::from_slice(BANK).expect("bank JSON");
    wrong_dimension["robust"]["hellinger"]["feature_mean"]
        .as_array_mut()
        .expect("feature mean")
        .pop();
    let wrong_dimension = serde_json::to_vec(&wrong_dimension).expect("serialize invalid bank");
    assert!(matches!(
        FingerprintBank::from_json(&wrong_dimension),
        Err(FingerprintError::InvalidBank(_))
    ));
}

#[test]
fn bank_rejects_model_order_mismatch() {
    let mut value: serde_json::Value = serde_json::from_slice(BANK).expect("bank JSON");
    value["robust"]["model_order"]
        .as_array_mut()
        .expect("model order")
        .swap(0, 1);
    let bytes = serde_json::to_vec(&value).expect("serialize mismatched bank");
    assert!(matches!(
        FingerprintBank::from_json(&bytes),
        Err(FingerprintError::InvalidBank(_))
    ));
}
