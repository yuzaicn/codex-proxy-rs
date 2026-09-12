use axum::body::to_bytes;
use gateway_api::admin::detection::{DetectionRecordHtmlQuery, detection_record_html_response};
use serde_json::json;

#[tokio::test]
async fn detection_record_html_response_is_sandboxed_and_uncached() {
    let response =
        detection_record_html_response("<!DOCTYPE html><html><body>ok</body></html>".to_owned());

    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers()["content-type"],
        "text/html; charset=utf-8"
    );
    assert_eq!(
        response.headers()["content-security-policy"],
        "sandbox allow-scripts"
    );
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert_eq!(
        response.headers()["cross-origin-resource-policy"],
        "same-origin"
    );
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    assert_eq!(
        to_bytes(response.into_body(), 1024)
            .await
            .expect("html response body"),
        "<!DOCTYPE html><html><body>ok</body></html>"
    );
}

#[test]
fn detection_record_html_query_accepts_only_positive_ids() {
    let valid: DetectionRecordHtmlQuery =
        serde_json::from_value(json!({ "id": 42 })).expect("decode html query");
    valid.validate().expect("validate html query");

    for id in [json!(null), json!(0), json!(-7)] {
        let invalid: DetectionRecordHtmlQuery =
            serde_json::from_value(json!({ "id": id })).expect("decode invalid html query");
        assert!(invalid.validate().is_err());
    }
}
