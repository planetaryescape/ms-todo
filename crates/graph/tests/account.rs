mod support;

use ms_todo_graph::auth::AuthError;
use serde_json::json;
use support::{Fixture, token};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, ResponseTemplate};

#[tokio::test]
async fn me_returns_the_account_with_the_bearer_token() {
    let fx = Fixture::new().await;
    Mock::given(method("GET"))
        .and(path("/v1.0/me"))
        .and(header("authorization", "Bearer access-for-r1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "userPrincipalName": "bk@outlook.com",
            "mail": null,
            "displayName": "BK"
        })))
        .mount(&fx.server)
        .await;

    let account = fx.auth.account(&token("r1", 3600)).await.expect("me");

    assert_eq!(account.sign_in_name(), Some("bk@outlook.com"));
    assert_eq!(account.display_name.as_deref(), Some("BK"));
}

#[tokio::test]
async fn falls_back_to_mail_without_a_user_principal_name() {
    let fx = Fixture::new().await;
    Mock::given(method("GET"))
        .and(path("/v1.0/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "mail": "bk@example.com" })))
        .mount(&fx.server)
        .await;

    let account = fx.auth.account(&token("r1", 3600)).await.expect("me");

    assert_eq!(account.sign_in_name(), Some("bk@example.com"));
}

#[tokio::test]
async fn unauthorized_means_sign_in_again() {
    let fx = Fixture::new().await;
    Mock::given(method("GET"))
        .and(path("/v1.0/me"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": { "code": "InvalidAuthenticationToken", "message": "expired" }
        })))
        .mount(&fx.server)
        .await;

    let error = fx.auth.account(&token("r1", 3600)).await.expect_err("401");

    assert!(matches!(error, AuthError::Expired), "{error:?}");
    assert_eq!(error.kind().exit_code(), 4);
}

#[tokio::test]
async fn graph_errors_keep_their_code_and_request_id() {
    let fx = Fixture::new().await;
    Mock::given(method("GET"))
        .and(path("/v1.0/me"))
        .respond_with(
            ResponseTemplate::new(503)
                .insert_header("request-id", "req-123")
                .set_body_json(json!({
                    "error": { "code": "serviceNotAvailable", "message": "try later" }
                })),
        )
        .mount(&fx.server)
        .await;

    let error = fx.auth.account(&token("r1", 3600)).await.expect_err("503");

    assert_eq!(error.graph_code(), Some("serviceNotAvailable"));
    assert_eq!(error.request_id(), Some("req-123"));
    assert_eq!(error.kind().exit_code(), 1);
}
