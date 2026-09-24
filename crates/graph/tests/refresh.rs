mod support;

use ms_todo_graph::auth::AuthError;
use serde_json::json;
use support::{Fixture, token, write_token_file};
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, Request, ResponseTemplate};

const TOKEN_PATH: &str = "/common/oauth2/v2.0/token";

fn refresh_mock(refresh_token: &str) -> wiremock::MockBuilder {
    Mock::given(method("POST"))
        .and(path(TOKEN_PATH))
        .and(body_string_contains("grant_type=refresh_token"))
        .and(body_string_contains(format!(
            "refresh_token={refresh_token}"
        )))
}

#[tokio::test]
async fn fresh_token_is_used_without_refreshing() {
    let fx = Fixture::new().await;
    let stored = token("r1", 3600);
    fx.write_token(&stored);
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&fx.server)
        .await;

    assert_eq!(fx.auth.valid_token().await.expect("token"), stored);
}

#[tokio::test]
async fn refreshes_near_expiry_and_saves_the_rotated_token() {
    let fx = Fixture::new().await;
    fx.write_token(&token("r1", 60));
    refresh_mock("r1")
        .and(body_string_contains("client_id=test-client"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "a2",
            "refresh_token": "r2",
            "expires_in": 3600
        })))
        .expect(1)
        .mount(&fx.server)
        .await;

    let refreshed = fx.auth.valid_token().await.expect("refreshed");

    assert_eq!(refreshed.access_token, "a2");
    assert_eq!(refreshed.refresh_token, "r2");
    // No `scope` in the response: the old scopes carry over.
    assert_eq!(refreshed.scopes, vec!["Tasks.ReadWrite".to_owned()]);
    assert_eq!(fx.read_token(), Some(refreshed));
}

#[tokio::test]
async fn keeps_the_old_refresh_token_when_the_response_has_none() {
    let fx = Fixture::new().await;
    fx.write_token(&token("r1", 60));
    refresh_mock("r1")
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "a2",
            "expires_in": 3600
        })))
        .mount(&fx.server)
        .await;

    let refreshed = fx.auth.valid_token().await.expect("refreshed");

    assert_eq!(refreshed.access_token, "a2");
    assert_eq!(refreshed.refresh_token, "r1");
    assert_eq!(fx.read_token().expect("saved").refresh_token, "r1");
}

#[tokio::test]
async fn uses_a_newer_stored_token_instead_of_spending_the_stale_one() {
    let fx = Fixture::new().await;
    let stale = token("r1", 60);
    let newer = token("r2", 3600);
    fx.write_token(&newer);
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&fx.server)
        .await;

    let used = fx.auth.refresh(&stale).await.expect("newer token");

    assert_eq!(used, newer);
}

#[tokio::test]
async fn invalid_grant_deletes_the_failed_credential() {
    let fx = Fixture::new().await;
    fx.write_token(&token("r1", 60));
    refresh_mock("r1")
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": "invalid_grant",
            "error_description": "AADSTS70008: The refresh token has expired"
        })))
        .mount(&fx.server)
        .await;

    let error = fx.auth.valid_token().await.expect_err("revoked");

    assert!(matches!(error, AuthError::Revoked), "{error:?}");
    assert_eq!(error.kind().exit_code(), 4);
    assert_eq!(fx.read_token(), None);
}

#[tokio::test]
async fn invalid_grant_keeps_a_credential_that_replaced_the_failed_one() {
    let fx = Fixture::new().await;
    fx.write_token(&token("r1", 60));
    let replacement = token("r2", 3600);
    let token_path = fx.token_path();
    let written = replacement.clone();
    // A new sign-in lands while the refresh is in flight.
    refresh_mock("r1")
        .respond_with(move |_: &Request| {
            write_token_file(&token_path, &written);
            ResponseTemplate::new(400).set_body_json(json!({ "error": "invalid_grant" }))
        })
        .mount(&fx.server)
        .await;

    let error = fx.auth.valid_token().await.expect_err("revoked");

    assert!(matches!(error, AuthError::Revoked), "{error:?}");
    assert_eq!(fx.read_token(), Some(replacement));
}

#[tokio::test]
async fn signed_out_is_auth_required() {
    let fx = Fixture::new().await;

    let error = fx.auth.valid_token().await.expect_err("no token");

    assert!(matches!(error, AuthError::NotSignedIn), "{error:?}");
    assert_eq!(error.kind().as_str(), "auth_required");
}

#[tokio::test]
async fn sign_out_deletes_the_token_and_is_idempotent() {
    let fx = Fixture::new().await;
    fx.write_token(&token("r1", 3600));

    assert!(fx.auth.sign_out().await.expect("first"));
    assert!(!fx.auth.sign_out().await.expect("second"));
    assert_eq!(fx.read_token(), None);
}
