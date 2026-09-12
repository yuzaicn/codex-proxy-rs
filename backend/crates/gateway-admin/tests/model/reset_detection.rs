use gateway_admin::model::reset_detection::ResetDetectionAccountScope;

#[test]
fn account_scope_values_round_trip_and_reject_unknown_values() {
    for (value, expected) in [
        ("all_non_error", ResetDetectionAccountScope::AllNonError),
        ("normal", ResetDetectionAccountScope::Normal),
        ("limited", ResetDetectionAccountScope::Limited),
    ] {
        assert_eq!(ResetDetectionAccountScope::parse(value), Some(expected));
        assert_eq!(expected.as_str(), value);
    }
    assert_eq!(ResetDetectionAccountScope::parse("other"), None);
}
