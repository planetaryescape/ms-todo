//! The write half of the HTTP client: what's resent and what isn't (D-028),
//! `If-Match`, and DELETE's 404.

mod support;

use std::sync::Arc;
use std::time::Duration;

use ms_todo_core::ErrorKind;
use ms_todo_graph::auth::{Authenticator, Endpoints};
use ms_todo_graph::{GraphClient, GraphError};
use serde_json::json;
use support::{token, write_token_file};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TASK: &str = "/v1.0/me/todo/lists/L1/tasks/T1";

async fn client(server: &MockServer, dir: &tempfile::TempDir) -> GraphClient {
    let endpoints = Endpoints {
        authority: format!("{}/common/oauth2/v2.0", server.uri()),
        graph: format!("{}/v1.0", server.uri()),
    };
    let auth = Authenticator::new(dir.path().join("auth"), endpoints.clone()).expect("auth");
    write_token_file(&auth.token_path(), &token("r1", 3600));
    GraphClient::new(Arc::new(auth), &endpoints.graph)
        .expect("client")
        .with_backoff_unit(Duration::from_millis(1))
}

async fn requests(server: &MockServer) -> usize {
    server.received_requests().await.unwrap_or_default().len()
}

#[tokio::test]
async fn an_absolute_patch_is_retried_after_a_5xx_and_carries_if_match() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let graph = client(&server, &dir).await;
    Mock::given(method("PATCH"))
        .and(path(TASK))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path(TASK))
        .and(header("If-Match", "W/\"e1\""))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "T1" })))
        .mount(&server)
        .await;

    let updated = graph
        .update_task("L1", "T1", &json!({ "title": "x" }), Some("W/\"e1\""), true)
        .await
        .expect("retried");

    assert_eq!(updated["id"], "T1");
    assert_eq!(requests(&server).await, 2);
}

#[tokio::test]
async fn a_non_idempotent_patch_is_outcome_unknown_after_a_5xx_or_408() {
    for status in [500, 503, 408] {
        let server = MockServer::start().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let graph = client(&server, &dir).await;
        Mock::given(method("PATCH"))
            .and(path(TASK))
            .respond_with(ResponseTemplate::new(status))
            .mount(&server)
            .await;

        let error = graph
            .update_task("L1", "T1", &json!({ "status": "completed" }), None, false)
            .await
            .expect_err("unknown");

        assert!(matches!(error, GraphError::OutcomeUnknown(_)), "{status}");
        assert_eq!(error.kind(), ErrorKind::OutcomeUnknown);
        assert_eq!(error.status(), Some(status));
        assert_eq!(requests(&server).await, 1, "{status}: sent once");
    }
}

#[tokio::test]
async fn a_create_is_refreshed_and_resent_after_a_401() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let graph = client(&server, &dir).await;
    Mock::given(method("POST"))
        .and(path("/common/oauth2/v2.0/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "fresh", "refresh_token": "r2", "expires_in": 3600,
            "scope": "Tasks.ReadWrite", "token_type": "Bearer"
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1.0/me/todo/lists/L1/tasks"))
        .and(header("Authorization", "Bearer access-for-r1"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1.0/me/todo/lists/L1/tasks"))
        .and(header("Authorization", "Bearer fresh"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "id": "T1" })))
        .mount(&server)
        .await;

    let created = graph
        .create_task("L1", &json!({ "title": "x" }))
        .await
        .expect("a 401 proves the create didn't run");
    assert_eq!(created["id"], "T1");
}

#[tokio::test]
async fn a_delete_that_finds_nothing_is_success() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let graph = client(&server, &dir).await;
    Mock::given(method("DELETE"))
        .and(path(TASK))
        .respond_with(
            ResponseTemplate::new(404).set_body_json(
                json!({ "error": { "code": "ErrorItemNotFound", "message": "gone" } }),
            ),
        )
        .mount(&server)
        .await;

    graph.delete_task("L1", "T1").await.expect("already gone");
}
