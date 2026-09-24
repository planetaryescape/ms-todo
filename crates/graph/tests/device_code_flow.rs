mod support;

use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ms_todo_graph::auth::{AuthError, SCOPES};
use serde_json::json;
use support::{CLIENT_ID, Fixture};
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, Request, ResponseTemplate};

const DEVICE_CODE_PATH: &str = "/common/oauth2/v2.0/devicecode";
const TOKEN_PATH: &str = "/common/oauth2/v2.0/token";

async fn mount_device_code(fx: &Fixture, expires_in: u64) {
    Mock::given(method("POST"))
        .and(path(DEVICE_CODE_PATH))
        .and(body_string_contains("client_id=test-client"))
        .and(body_string_contains("offline_access"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "device_code": "dev-code",
            "user_code": "ABCD-EFGH",
            "verification_uri": "https://microsoft.com/devicelogin",
            "expires_in": expires_in,
            "interval": 1,
            "message": "To sign in, use a web browser..."
        })))
        .mount(&fx.server)
        .await;
}

fn oauth_error(code: &str) -> ResponseTemplate {
    ResponseTemplate::new(400).set_body_json(json!({
        "error": code,
        "error_description": format!("AADSTS: {code}")
    }))
}

fn token_success() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "token_type": "Bearer",
        "access_token": "access-1",
        "refresh_token": "refresh-1",
        "expires_in": 3600,
        "scope": "Tasks.ReadWrite MailboxSettings.ReadWrite User.Read"
    }))
}

/// Answers the first `times` polls with `response`, in mount order.
async fn mount_poll(fx: &Fixture, response: ResponseTemplate, times: u64) {
    Mock::given(method("POST"))
        .and(path(TOKEN_PATH))
        .and(body_string_contains("device_code=dev-code"))
        .respond_with(response)
        .up_to_n_times(times)
        .mount(&fx.server)
        .await;
}

async fn sign_in(fx: &Fixture) -> Result<ms_todo_graph::auth::StoredToken, AuthError> {
    let code = fx.auth.start_device_flow(CLIENT_ID).await?;
    assert_eq!(code.user_code, "ABCD-EFGH");
    fx.auth.finish_device_flow(CLIENT_ID, &code).await
}

#[tokio::test]
async fn keeps_polling_while_pending_then_saves_the_token() {
    let fx = Fixture::new().await;
    mount_device_code(&fx, 900).await;
    mount_poll(&fx, oauth_error("authorization_pending"), 3).await;
    mount_poll(&fx, token_success(), 1).await;

    let token = sign_in(&fx).await.expect("signed in");

    assert_eq!(token.refresh_token, "refresh-1");
    assert_eq!(token.client_id, CLIENT_ID);
    assert!(token.scopes.contains(&"User.Read".to_owned()));
    assert_eq!(fx.read_token(), Some(token));
}

#[tokio::test]
async fn slow_down_adds_five_intervals_and_keeps_polling() {
    let fx = Fixture::new().await;
    mount_device_code(&fx, 900).await;
    let polls = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&polls);
    Mock::given(method("POST"))
        .and(path(TOKEN_PATH))
        .respond_with(move |_: &Request| {
            let mut seen = seen.lock().expect("lock");
            seen.push(Instant::now());
            if seen.len() == 1 {
                oauth_error("slow_down")
            } else {
                token_success()
            }
        })
        .mount(&fx.server)
        .await;

    sign_in(&fx).await.expect("signed in after slow_down");

    // The server said interval 1; after slow_down it's 1 + 5 = 6 units of 2 ms.
    // Sleeps never end early, so this lower bound can't flake.
    let polls = polls.lock().expect("lock");
    assert_eq!(polls.len(), 2);
    assert!(polls[1] - polls[0] >= Duration::from_millis(12));
}

#[tokio::test]
async fn expired_code_stops_with_a_sign_in_error() {
    let fx = Fixture::new().await;
    mount_device_code(&fx, 900).await;
    mount_poll(&fx, oauth_error("authorization_pending"), 1).await;
    mount_poll(&fx, oauth_error("expired_token"), 1).await;

    let error = sign_in(&fx).await.expect_err("expired");
    assert!(matches!(error, AuthError::DeviceCodeExpired), "{error:?}");
    assert_eq!(error.kind().exit_code(), 4);
    assert_eq!(fx.read_token(), None);
}

#[tokio::test]
async fn gives_up_locally_once_the_code_has_expired() {
    let fx = Fixture::new().await;
    // 20 units of 2 ms: the code dies long before the server stops saying pending.
    mount_device_code(&fx, 20).await;
    mount_poll(&fx, oauth_error("authorization_pending"), 1000).await;

    let error = sign_in(&fx).await.expect_err("expired locally");
    assert!(matches!(error, AuthError::DeviceCodeExpired), "{error:?}");
}

#[tokio::test]
async fn declined_stops_cleanly() {
    let fx = Fixture::new().await;
    mount_device_code(&fx, 900).await;
    mount_poll(&fx, oauth_error("access_denied"), 1).await;

    let error = sign_in(&fx).await.expect_err("declined");
    assert!(matches!(error, AuthError::Declined), "{error:?}");
}

#[tokio::test]
async fn tolerates_transient_failures_while_polling() {
    let fx = Fixture::new().await;
    mount_device_code(&fx, 900).await;
    // A gateway error page is not a token response: a transport-level failure.
    mount_poll(
        &fx,
        ResponseTemplate::new(502).set_body_string("<html>bad gateway</html>"),
        5,
    )
    .await;
    mount_poll(&fx, token_success(), 1).await;

    sign_in(&fx)
        .await
        .expect("five failures in a row are tolerated");
}

#[tokio::test]
async fn six_transient_failures_in_a_row_is_a_network_error() {
    let fx = Fixture::new().await;
    mount_device_code(&fx, 900).await;
    mount_poll(
        &fx,
        ResponseTemplate::new(502).set_body_string("bad gateway"),
        6,
    )
    .await;
    mount_poll(&fx, token_success(), 1).await;

    let error = sign_in(&fx).await.expect_err("sixth failure gives up");
    assert!(matches!(error, AuthError::Network(_)), "{error:?}");
    assert_eq!(error.kind().exit_code(), 1);
}

#[tokio::test]
async fn failed_device_code_request_reports_the_oauth_error() {
    let fx = Fixture::new().await;
    Mock::given(method("POST"))
        .and(path(DEVICE_CODE_PATH))
        .respond_with(oauth_error("invalid_client"))
        .mount(&fx.server)
        .await;

    let error = fx
        .auth
        .start_device_flow(CLIENT_ID)
        .await
        .expect_err("bad client");
    assert_eq!(error.graph_code(), Some("invalid_client"));
}

#[tokio::test]
async fn token_file_is_0600_in_a_0700_directory() {
    let fx = Fixture::new().await;
    mount_device_code(&fx, 900).await;
    mount_poll(&fx, token_success(), 1).await;

    sign_in(&fx).await.expect("signed in");

    let path = fx.token_path();
    let file_mode = std::fs::metadata(&path)
        .expect("token")
        .permissions()
        .mode();
    let dir_mode = std::fs::metadata(path.parent().expect("dir"))
        .expect("auth dir")
        .permissions()
        .mode();
    assert_eq!(file_mode & 0o777, 0o600);
    assert_eq!(dir_mode & 0o777, 0o700);
}

#[test]
fn requests_the_graph_scopes_from_the_blueprint() {
    assert_eq!(
        SCOPES,
        "offline_access Tasks.ReadWrite MailboxSettings.ReadWrite User.Read"
    );
}
