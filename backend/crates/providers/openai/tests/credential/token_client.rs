use provider_openai::credential::token_client::{
    AuthorizationCodeExchangeError, AuthorizationCodeExchanger, AuthorizationCodeGrant,
    OpenAiTokenClient, RefreshFailure, TokenClientConfig, TokenRefresher,
};
use secrecy::{ExposeSecret, SecretString};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn refresh_and_code_exchange_use_the_selected_account_proxy() {
    let direct = MockServer::start().await;
    let proxy_a = MockServer::start().await;
    let proxy_b = MockServer::start().await;
    for (proxy, access) in [(&proxy_a, "exit-a-access"), (&proxy_b, "exit-b-access")] {
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": access,
                "refresh_token": "rotated-refresh",
                "id_token": "header.e30.signature"
            })))
            .expect(2)
            .mount(proxy)
            .await;
    }
    let client = client(&direct);
    for (proxy, expected) in [(&proxy_a, "exit-a-access"), (&proxy_b, "exit-b-access")] {
        let proxy = gateway_core::account::OutboundProxy::parse(&proxy.uri()).unwrap();
        let refreshed = client
            .refresh_with_proxy("initial-refresh", Some(&proxy))
            .await
            .unwrap();
        assert_eq!(refreshed.access_token.as_deref(), Some(expected));
        let exchanged = client
            .exchange_with_proxy(
                AuthorizationCodeGrant {
                    code: SecretString::from("test-code"),
                    code_verifier: SecretString::from("test-verifier"),
                },
                Some(&proxy),
            )
            .await
            .unwrap();
        assert_eq!(exchanged.secret.access_token.expose_secret(), expected);
    }
    assert!(direct.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn refresh_tracks_the_online_profile_without_contaminating_code_exchange() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "audit-access", "refresh_token": "audit-refresh", "id_token": "header.e30.signature"
        })))
        .expect(4)
        .mount(&server)
        .await;
    let profile = provider_openai::OpenAiConfig::default().wire_profile_state();
    let client = provider_openai::credential::token_client::openai_token_client(
        TokenClientConfig {
            client_id: "audit-client".to_owned(),
            token_endpoint: format!("{}/oauth/token", server.uri()),
        },
        profile.clone(),
    )
    .expect("production auth transport");
    for (core, desktop, build) in [
        ("1.2.3", "26.800.10000", "8000"),
        ("1.2.4", "26.901.10000", "8109"),
    ] {
        profile.update_bundled_release(
            &provider_openai::transport::profile::CodexBundledReleaseProfile {
                codex_version: core.to_owned(),
                desktop_version: desktop.to_owned(),
                desktop_build: build.to_owned(),
                verified_at: chrono::Utc::now(),
            },
        );
        client.refresh("audit-refresh").await.expect("refresh");
        client
            .exchange_authorization_code(AuthorizationCodeGrant {
                code: SecretString::from("audit-code"),
                code_verifier: SecretString::from("audit-verifier"),
            })
            .await
            .expect("code exchange");
        let requests = server.received_requests().await.expect("requests");
        let refresh = &requests[requests.len() - 2];
        let exchange = &requests[requests.len() - 1];
        assert_eq!(
            refresh.headers["user-agent"],
            profile.snapshot().user_agent()
        );
        assert_eq!(refresh.headers["originator"], "Codex Desktop");
        assert_eq!(refresh.headers["content-type"], "application/json");
        assert_eq!(
            exchange.headers["content-type"],
            "application/x-www-form-urlencoded"
        );
        for name in [
            "originator",
            "user-agent",
            "version",
            "x-openai-internal-codex-residency",
        ] {
            assert!(!exchange.headers.contains_key(name), "raw exchange: {name}");
        }
        for name in ["authorization", "chatgpt-account-id", "version"] {
            assert!(!refresh.headers.contains_key(name), "refresh: {name}");
        }
    }
}

fn client(server: &MockServer) -> OpenAiTokenClient {
    OpenAiTokenClient::new(
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("test HTTP client"),
        TokenClientConfig {
            client_id: "test-public-client".to_owned(),
            token_endpoint: format!("{}/oauth/token", server.uri()),
        },
        provider_openai::OpenAiConfig::default().wire_profile_state(),
    )
}

#[tokio::test]
async fn oversized_chunked_oauth_response_should_fail_closed_and_redact_body() {
    let server = MockServer::start().await;
    let marker = "oauth-secret-response-marker";
    let body = format!(
        "{{\"error\":\"invalid_grant\",\"marker\":\"{marker}\",\"padding\":\"{}\"}}",
        "x".repeat(70 * 1024)
    );
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(
            ResponseTemplate::new(400)
                .insert_header("transfer-encoding", "chunked")
                .set_body_string(body),
        )
        .expect(1)
        .mount(&server)
        .await;

    let failure = client(&server)
        .refresh("refresh-secret-request-marker")
        .await
        .expect_err("oversized response must fail closed before body classification");
    let diagnostic = format!("{failure:?} {failure}");

    assert!(matches!(
        failure,
        RefreshFailure::Transport { upstream: None, .. }
    ));
    assert!(!diagnostic.contains(marker));
    assert!(!diagnostic.contains("refresh-secret-request-marker"));
}

#[tokio::test]
async fn bounded_oauth_response_should_parse_the_official_rotated_token_fields() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "access-rotated",
            "refresh_token": "refresh-rotated",
            "id_token": "header.e30.signature",
            "expires_in": 3600
        })))
        .expect(1)
        .mount(&server)
        .await;

    let tokens = client(&server)
        .refresh("refresh-initial")
        .await
        .expect("bounded response");

    assert_eq!(tokens.access_token.as_deref(), Some("access-rotated"));
    assert_eq!(tokens.refresh_token.as_deref(), Some("refresh-rotated"));
    assert_eq!(tokens.id_token.as_deref(), Some("header.e30.signature"));
}

#[tokio::test]
async fn refresh_response_should_reject_a_malformed_rotated_id_token() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "access-rotated",
            "id_token": "not-a-jwt"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let failure = client(&server)
        .refresh("refresh-initial")
        .await
        .expect_err("malformed ID token must not replace the stored token set");

    assert!(matches!(
        failure,
        RefreshFailure::Transport { upstream: None, .. }
    ));
}

#[tokio::test]
async fn refresh_response_should_allow_all_rotated_tokens_to_be_omitted() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .expect(1)
        .mount(&server)
        .await;

    let tokens = client(&server)
        .refresh("refresh-initial")
        .await
        .expect("official refresh response fields are optional");

    assert!(tokens.access_token.is_none());
    assert!(tokens.refresh_token.is_none());
    assert!(tokens.id_token.is_none());
}

#[tokio::test]
async fn refresh_response_keeps_the_upstream_rotated_token_verbatim() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "access-rotated",
            "refresh_token": " ",
            "expires_in": 3600
        })))
        .expect(1)
        .mount(&server)
        .await;

    let tokens = client(&server)
        .refresh("refresh-initial")
        .await
        .expect("upstream response is accepted");

    assert_eq!(tokens.refresh_token.as_deref(), Some(" "));
}

#[tokio::test]
async fn refresh_should_exchange_the_official_json_fields() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "new-access",
            "refresh_token": "new-refresh",
            "expires_in": 3600
        })))
        .expect(1)
        .mount(&server)
        .await;

    client(&server)
        .refresh("refresh secret")
        .await
        .expect("refresh succeeds");
    let requests = server.received_requests().await.expect("received request");
    let body: serde_json::Value =
        serde_json::from_slice(&requests[0].body).expect("JSON request body");

    assert_eq!(
        body,
        serde_json::json!({
            "client_id": "test-public-client",
            "grant_type": "refresh_token",
            "refresh_token": "refresh secret"
        })
    );
    assert_eq!(
        requests[0]
            .headers
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );
}

#[tokio::test]
async fn authorization_code_exchange_should_require_bounded_oidc_token_set_and_pkce_form() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "header.access.signature",
            "refresh_token": "refresh-token",
            "id_token": "header.id.signature",
            "token_type": "Bearer",
            "expires_in": 3600
        })))
        .expect(1)
        .mount(&server)
        .await;

    let tokens = client(&server)
        .exchange_authorization_code(AuthorizationCodeGrant {
            code: SecretString::from("authorization code"),
            code_verifier: SecretString::from("pkce-verifier-secret"),
        })
        .await
        .expect("exchange bounded OIDC token set");
    let requests = server.received_requests().await.expect("received request");
    let body = String::from_utf8(requests[0].body.clone()).expect("form body");

    assert_eq!(
        tokens.secret.access_token.expose_secret(),
        "header.access.signature"
    );
    assert_eq!(tokens.id_token.expose_secret(), "header.id.signature");
    assert!(body.contains("grant_type=authorization_code"));
    assert!(body.contains("client_id=test-public-client"));
    assert!(body.contains("code=authorization+code"));
    assert!(body.contains("code_verifier=pkce-verifier-secret"));
    assert!(body.contains("redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback"));
}

#[tokio::test]
async fn authorization_code_exchange_keeps_upstream_access_and_refresh_tokens_verbatim() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": " ",
            "refresh_token": "",
            "id_token": "header.payload.signature"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let tokens = client(&server)
        .exchange_authorization_code(AuthorizationCodeGrant {
            code: SecretString::from("authorization-code"),
            code_verifier: SecretString::from("pkce-verifier"),
        })
        .await
        .expect("token endpoint response is trusted structurally");

    assert_eq!(tokens.secret.access_token.expose_secret(), " ");
    assert_eq!(
        tokens
            .secret
            .refresh_token
            .as_ref()
            .expect("required response field")
            .expose_secret(),
        ""
    );
}

#[tokio::test]
async fn authorization_code_exchange_requires_id_token_and_json_response() {
    let missing_id = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "header.access.signature",
            "refresh_token": "refresh-token",
            "token_type": "Bearer",
            "expires_in": 3600
        })))
        .mount(&missing_id)
        .await;
    let non_json = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not-json"))
        .mount(&non_json)
        .await;

    for server in [&missing_id, &non_json] {
        let error = client(server)
            .exchange_authorization_code(AuthorizationCodeGrant {
                code: SecretString::from("code-secret"),
                code_verifier: SecretString::from("verifier-secret"),
            })
            .await
            .expect_err("invalid token response fails closed");
        assert_eq!(error, AuthorizationCodeExchangeError::Rejected);
    }
}

#[tokio::test]
async fn refresh_token_reuse_should_be_classified_as_invalid_grant() {
    let upstream_message = "Your refresh token has already been used to generate a new access token. Please try signing in again.";
    let failure = refresh_failure(
        400,
        r#"{"error":{"message":"Your refresh token has already been used to generate a new access token. Please try signing in again.","type":"invalid_request_error","param":null,"code":"refresh_token_reused"}}"#,
    )
    .await;

    let RefreshFailure::InvalidGrant { message, upstream } = failure else {
        panic!("refresh-token reuse must be terminal");
    };
    assert_eq!(message.as_deref(), Some(upstream_message));
    let upstream = upstream.expect("complete upstream failure");
    assert_eq!(upstream.status(), 400);
    assert_eq!(upstream.code(), Some("refresh_token_reused"));
    assert_eq!(upstream.error_type(), Some("invalid_request_error"));
    assert!(upstream.body().contains(r#""code":"refresh_token_reused""#));
}

#[tokio::test]
async fn generic_invalid_grant_should_remain_transient_like_official_codex() {
    let body = r#"{"error":"invalid_grant","error_description":"not the official refresh error message field"}"#;
    let failure = refresh_failure(400, body).await;
    let diagnostic = format!("{failure:?} {failure}");

    let RefreshFailure::Transport { message, upstream } = failure else {
        panic!("unknown official refresh code must remain transient");
    };
    assert_eq!(message, None);
    let upstream = upstream.expect("complete upstream failure");
    assert_eq!(upstream.code(), Some("invalid_grant"));
    assert_eq!(upstream.body(), body);
    assert!(!diagnostic.contains("error_description"));
}

#[tokio::test]
async fn unauthorized_should_preserve_upstream_details_and_remain_retryable() {
    let body = r#"{
        "error": {
            "message": "Invalid refresh token.",
            "type": "invalid_request_error",
            "param": null,
            "code": "invalid_refresh_token"
        }
    }"#;
    let failure = refresh_failure(401, body).await;
    assert!(failure.is_retryable());

    let RefreshFailure::Transport { message, upstream } = failure else {
        panic!("production policy gives every 401 a bounded recovery window");
    };
    assert_eq!(message.as_deref(), Some("Invalid refresh token."));
    let upstream = upstream.expect("complete upstream failure");
    assert_eq!(upstream.status(), 401);
    assert_eq!(upstream.code(), Some("invalid_refresh_token"));
    assert_eq!(upstream.error_type(), Some("invalid_request_error"));
    assert_eq!(upstream.body(), body);
}

#[tokio::test]
async fn unauthorized_should_back_off_even_with_a_recognized_refresh_code() {
    let failure = refresh_failure(
        401,
        r#"{"error":{"code":"refresh_token_expired","message":"Refresh token expired."}}"#,
    )
    .await;

    assert_transport_failure(
        &failure,
        401,
        Some("Refresh token expired."),
        r#"{"error":{"code":"refresh_token_expired","message":"Refresh token expired."}}"#,
    );
}

#[tokio::test]
async fn deactivated_account_should_be_classified_as_banned() {
    let failure = refresh_failure(
        403,
        r#"{"error":{"code":"access_denied","message":"account has been deactivated"}}"#,
    )
    .await;

    let RefreshFailure::Banned { message, upstream } = failure else {
        panic!("deactivated account must be banned");
    };
    assert_eq!(message.as_deref(), Some("account has been deactivated"));
    assert_eq!(upstream.expect("complete upstream failure").status(), 403);
}

#[tokio::test]
async fn generic_banned_text_should_not_impersonate_the_deactivation_contract() {
    let failure = refresh_failure(403, "account is banned").await;

    assert_transport_failure(&failure, 403, None, "account is banned");
}

#[tokio::test]
async fn unregistered_disabled_account_text_should_remain_a_transport_failure() {
    let failure = refresh_failure(400, "account disabled").await;

    assert_transport_failure(&failure, 400, None, "account disabled");
}

#[tokio::test]
async fn quota_text_should_not_disable_the_oauth_credential() {
    let failure = refresh_failure(400, "quota exceeded").await;

    assert_transport_failure(&failure, 400, None, "quota exceeded");
}

#[tokio::test]
async fn token_revoked_text_without_invalid_grant_should_remain_temporary() {
    let failure = refresh_failure(400, "token_revoked").await;

    assert_transport_failure(&failure, 400, None, "token_revoked");
}

#[tokio::test]
async fn recognized_refresh_code_should_be_permanent_on_non_unauthorized_status() {
    for status in [500, 502, 503, 429] {
        let failure = refresh_failure(
            status,
            r#"{"error":{"code":"refresh_token_expired","message":"Refresh token expired."}}"#,
        )
        .await;
        assert!(
            matches!(&failure, RefreshFailure::InvalidGrant { .. }),
            "official refresh code must take precedence for status {status}"
        );
        assert!(!failure.is_retryable());
    }
}

#[tokio::test]
async fn recognized_refresh_codes_should_all_be_permanent() {
    for code in [
        "refresh_token_expired",
        "refresh_token_reused",
        "refresh_token_invalidated",
    ] {
        let body = format!(r#"{{"error":{{"code":"{code}","message":"Rejected."}}}}"#);
        let failure = refresh_failure(400, &body).await;

        assert_eq!(failure.classification(), "invalid-grant", "code {code}");
        assert!(!failure.is_retryable(), "code {code}");
    }
}

#[tokio::test]
async fn rate_limit_should_be_retryable_and_preserve_bounded_retry_after() {
    let body = r#"{"error":{"code":"temporarily_unavailable","message":"Try again."}}"#;
    for (header, expected) in [
        (Some("0"), Some(Duration::ZERO)),
        (None, None),
        (Some("42"), Some(Duration::from_secs(42))),
        (Some("999"), Some(Duration::from_secs(300))),
    ] {
        let mut response = ResponseTemplate::new(429).set_body_string(body);
        if let Some(header) = header {
            response = response.insert_header("retry-after", header);
        }
        let failure = refresh_failure_with_response(response).await;
        let RefreshFailure::UpstreamRateLimited {
            message,
            upstream,
            retry_after,
        } = &failure
        else {
            panic!("429 must classify as an upstream rate limit");
        };
        assert_eq!(message.as_deref(), Some("Try again."));
        assert_eq!(*retry_after, expected);
        assert_eq!(upstream.as_deref().expect("upstream details").status(), 429);
        assert_eq!(failure.classification(), "upstream-rate-limited");
        assert!(failure.is_retryable());
    }
}

#[tokio::test]
async fn rate_limit_should_normalize_http_date_retry_after_with_ceiling() {
    let retry_at = SystemTime::now() + Duration::from_millis(1_500);
    let retry_after = httpdate::fmt_http_date(retry_at);
    let response = ResponseTemplate::new(429)
        .insert_header("retry-after", retry_after)
        .set_body_string(r#"{"error":{"message":"Try again."}}"#);
    let failure = refresh_failure_with_response(response).await;

    let RefreshFailure::UpstreamRateLimited { retry_after, .. } = failure else {
        panic!("429 must classify as an upstream rate limit");
    };
    assert!(matches!(retry_after, Some(delay) if (1..=3).contains(&delay.as_secs())));
}

#[tokio::test]
async fn server_errors_should_be_retryable_upstream_unavailable_failures() {
    let body = r#"{"error":{"code":"temporarily_unavailable","message":"Try again."}}"#;
    for status in [500, 502, 503] {
        let failure = refresh_failure(status, body).await;
        let RefreshFailure::UpstreamUnavailable { message, upstream } = &failure else {
            panic!("status {status} must classify as upstream unavailable");
        };
        assert_eq!(message.as_deref(), Some("Try again."));
        assert_eq!(
            upstream.as_deref().expect("upstream details").status(),
            status
        );
        assert_eq!(failure.classification(), "upstream-unavailable");
        assert!(failure.is_retryable());
    }
}

#[tokio::test]
async fn timeout_should_remain_an_ambiguous_retryable_transport_failure() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_millis(200))
                .set_body_json(serde_json::json!({"access_token": "too-late"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let timeout_client = OpenAiTokenClient::new(
        reqwest::Client::builder()
            .timeout(Duration::from_millis(20))
            .build()
            .expect("test HTTP client"),
        TokenClientConfig {
            client_id: "test-public-client".to_owned(),
            token_endpoint: format!("{}/oauth/token", server.uri()),
        },
        provider_openai::OpenAiConfig::default().wire_profile_state(),
    );

    let failure = timeout_client
        .refresh("refresh-secret")
        .await
        .expect_err("timeout must fail");

    assert!(matches!(
        &failure,
        RefreshFailure::Transport { upstream: None, .. }
    ));
    assert_eq!(failure.classification(), "transport-ambiguous");
    assert!(failure.is_retryable());
}

fn assert_transport_failure(
    failure: &RefreshFailure,
    status: u16,
    message: Option<&str>,
    body: &str,
) {
    let RefreshFailure::Transport {
        message: actual_message,
        upstream,
    } = failure
    else {
        panic!("status {status} must classify as transient");
    };
    assert_eq!(actual_message.as_deref(), message);
    let upstream = upstream.as_deref().expect("complete upstream failure");
    assert_eq!(upstream.status(), status);
    assert_eq!(upstream.body(), body);
}

async fn refresh_failure(status: u16, body: &str) -> RefreshFailure {
    refresh_failure_with_response(ResponseTemplate::new(status).set_body_string(body)).await
}

async fn refresh_failure_with_response(response: ResponseTemplate) -> RefreshFailure {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(response)
        .expect(1)
        .mount(&server)
        .await;

    client(&server)
        .refresh("refresh-secret")
        .await
        .expect_err("refresh must fail")
}
use std::time::{Duration, SystemTime};
