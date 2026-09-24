mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use ms_todo_core::ErrorKind;
use ms_todo_graph::auth::{AuthError, Authenticator, Endpoints};
use ms_todo_graph::{GraphClient, GraphError};
use serde_json::{Value, json};
use support::{token, write_token_file};
use wiremock::matchers::{header, method, path, query_param, query_param_is_missing};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TASKS: &str = "/v1.0/me/todo/lists/L1/tasks";
const PREFER: &str = "odata.maxpagesize=200";

struct Graph {
    server: MockServer,
    _dir: tempfile::TempDir,
    client: GraphClient,
}

impl Graph {
    /// A signed-in client whose token is good for an hour, so only a 401
    /// makes it refresh.
    async fn new() -> Self {
        let server = MockServer::start().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let endpoints = Endpoints {
            authority: format!("{}/common/oauth2/v2.0", server.uri()),
            graph: format!("{}/v1.0", server.uri()),
        };
        let auth = Authenticator::new(dir.path().join("auth"), endpoints.clone()).expect("auth");
        write_token_file(&auth.token_path(), &token("r1", 3600));
        let client = GraphClient::new(Arc::new(auth), &endpoints.graph)
            .expect("client")
            .with_backoff_unit(Duration::from_millis(1));
        Self {
            server,
            _dir: dir,
            client,
        }
    }

    fn next_link(&self, skip: usize) -> String {
        format!("{}{TASKS}?$skip={skip}", self.server.uri())
    }
}

fn tasks(from: usize, count: usize) -> Vec<Value> {
    (from..from + count)
        .map(|n| json!({ "id": format!("T{n}"), "title": format!("task {n}") }))
        .collect()
}

fn page(items: Vec<Value>, next: Option<String>) -> ResponseTemplate {
    let mut body = json!({ "value": items });
    if let Some(next) = next {
        body["@odata.nextLink"] = Value::String(next);
    }
    ResponseTemplate::new(200).set_body_json(body)
}

fn graph_error(status: u16, code: &str) -> ResponseTemplate {
    ResponseTemplate::new(status)
        .insert_header("request-id", "req-123")
        .set_body_json(json!({ "error": { "code": code, "message": "details" } }))
}

#[tokio::test]
async fn follows_every_next_link_and_resends_prefer_on_each_page() {
    let graph = Graph::new().await;
    // Four pages, a short one mid-stream, and an empty last page: none of
    // them may be mistaken for the end while a nextLink is present (P1).
    let pages = [
        (None, tasks(0, 200), Some(200)),
        (Some("200"), tasks(200, 200), Some(400)),
        (Some("400"), tasks(400, 7), Some(407)),
        (Some("407"), tasks(407, 200), Some(607)),
        (Some("607"), Vec::new(), None),
    ];
    for (skip, items, next) in pages {
        let mock = Mock::given(method("GET"))
            .and(path(TASKS))
            .and(header("Prefer", PREFER))
            .and(header("Authorization", "Bearer access-for-r1"));
        let mock = match skip {
            Some(skip) => mock.and(query_param("$skip", skip)),
            None => mock.and(query_param_is_missing("$skip")),
        };
        mock.respond_with(page(items, next.map(|skip| graph.next_link(skip))))
            .expect(1)
            .mount(&graph.server)
            .await;
    }

    let items = graph.client.list_tasks("L1").await.expect("tasks");

    assert_eq!(items.len(), 607);
    assert_eq!(items[0]["id"], "T0");
    assert_eq!(items[606]["id"], "T606");
    let requests = graph.server.received_requests().await.expect("recorded");
    assert_eq!(requests.len(), 5);
    assert!(requests.iter().all(|request| {
        request
            .headers
            .get("Prefer")
            .is_some_and(|value| value == PREFER)
    }));
}

#[tokio::test]
async fn a_429_waits_for_retry_after_then_succeeds() {
    let graph = Graph::new().await;
    Mock::given(method("GET"))
        .and(path(TASKS))
        .respond_with(graph_error(429, "activityLimitReached").insert_header("Retry-After", "1"))
        .up_to_n_times(1)
        .mount(&graph.server)
        .await;
    Mock::given(method("GET"))
        .and(path(TASKS))
        .respond_with(page(tasks(0, 2), None))
        .expect(1)
        .mount(&graph.server)
        .await;

    let started = Instant::now();
    let items = graph.client.list_tasks("L1").await.expect("tasks");

    assert_eq!(items.len(), 2);
    assert!(
        started.elapsed() >= Duration::from_secs(1),
        "Retry-After ignored"
    );
}

#[tokio::test]
async fn a_sustained_429_is_rate_limited_with_the_wait() {
    let graph = Graph::new().await;
    Mock::given(method("GET"))
        .and(path(TASKS))
        .respond_with(graph_error(429, "activityLimitReached").insert_header("Retry-After", "600"))
        .expect(1)
        .mount(&graph.server)
        .await;

    let error = graph.client.list_tasks("L1").await.expect_err("limited");

    assert_eq!(error.kind(), ErrorKind::RateLimited);
    assert!(
        matches!(error, GraphError::RateLimited { retry_after: Some(wait), .. } if wait == Duration::from_secs(600))
    );
    assert_eq!(error.request_id(), Some("req-123"));
}

#[tokio::test]
async fn server_errors_are_retried_then_succeed() {
    let graph = Graph::new().await;
    Mock::given(method("GET"))
        .and(path(TASKS))
        .respond_with(graph_error(503, "serviceNotAvailable"))
        .up_to_n_times(2)
        .expect(2)
        .mount(&graph.server)
        .await;
    Mock::given(method("GET"))
        .and(path(TASKS))
        .respond_with(page(tasks(0, 1), None))
        .expect(1)
        .mount(&graph.server)
        .await;

    let items = graph.client.list_tasks("L1").await.expect("tasks");

    assert_eq!(items.len(), 1);
}

#[tokio::test]
async fn server_errors_give_up_after_three_retries_with_code_and_request_id() {
    let graph = Graph::new().await;
    Mock::given(method("GET"))
        .and(path(TASKS))
        .respond_with(graph_error(500, "generalException"))
        .expect(4)
        .mount(&graph.server)
        .await;

    let error = graph.client.list_tasks("L1").await.expect_err("5xx");

    assert_eq!(error.kind(), ErrorKind::Api);
    assert_eq!(error.graph_code(), Some("generalException"));
    assert_eq!(error.request_id(), Some("req-123"));
}

#[tokio::test]
async fn a_401_refreshes_once_and_retries_with_the_new_token() {
    let graph = Graph::new().await;
    Mock::given(method("GET"))
        .and(path(TASKS))
        .and(header("Authorization", "Bearer access-for-r1"))
        .respond_with(graph_error(401, "InvalidAuthenticationToken"))
        .expect(1)
        .mount(&graph.server)
        .await;
    Mock::given(method("POST"))
        .and(path("/common/oauth2/v2.0/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "a2",
            "refresh_token": "r2",
            "expires_in": 3600
        })))
        .expect(1)
        .mount(&graph.server)
        .await;
    Mock::given(method("GET"))
        .and(path(TASKS))
        .and(header("Authorization", "Bearer a2"))
        .respond_with(page(tasks(0, 3), None))
        .expect(1)
        .mount(&graph.server)
        .await;

    let items = graph.client.list_tasks("L1").await.expect("tasks");

    assert_eq!(items.len(), 3);
}

#[tokio::test]
async fn a_second_401_after_the_refresh_is_auth_expired() {
    let graph = Graph::new().await;
    Mock::given(method("GET"))
        .and(path(TASKS))
        .respond_with(graph_error(401, "InvalidAuthenticationToken"))
        .expect(2)
        .mount(&graph.server)
        .await;
    Mock::given(method("POST"))
        .and(path("/common/oauth2/v2.0/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "a2",
            "refresh_token": "r2",
            "expires_in": 3600
        })))
        .expect(1)
        .mount(&graph.server)
        .await;

    let error = graph.client.list_tasks("L1").await.expect_err("expired");

    assert!(matches!(error, GraphError::Auth(AuthError::Expired)));
    assert_eq!(error.kind().exit_code(), 4);
}

#[tokio::test]
async fn graph_errors_are_parsed_into_typed_errors() {
    let graph = Graph::new().await;
    Mock::given(method("GET"))
        .and(path(TASKS))
        .respond_with(graph_error(404, "ErrorItemNotFound"))
        .expect(1)
        .mount(&graph.server)
        .await;

    let error = graph.client.list_tasks("L1").await.expect_err("404");

    assert_eq!(error.kind(), ErrorKind::NotFound);
    assert_eq!(error.graph_code(), Some("ErrorItemNotFound"));
    assert_eq!(error.request_id(), Some("req-123"));
    assert!(error.to_string().contains("details"), "{error}");
}

#[tokio::test]
async fn hitting_the_page_cap_is_an_error_not_a_partial_result() {
    let mut graph = Graph::new().await;
    graph.client = graph.client.with_max_pages(2);
    Mock::given(method("GET"))
        .and(path(TASKS))
        .respond_with(page(tasks(0, 1), Some(graph.next_link(1))))
        .mount(&graph.server)
        .await;

    let error = graph.client.list_tasks("L1").await.expect_err("cap");

    assert!(
        matches!(error, GraphError::PageLimit { pages: 2, .. }),
        "{error}"
    );
}

#[tokio::test]
async fn a_next_link_to_another_host_is_refused() {
    let graph = Graph::new().await;
    Mock::given(method("GET"))
        .and(path(TASKS))
        .respond_with(page(
            tasks(0, 1),
            Some("https://attacker.example/v1.0/me/todo/lists/L1/tasks?$skip=1".into()),
        ))
        .mount(&graph.server)
        .await;

    let error = graph.client.list_tasks("L1").await.expect_err("refused");

    assert_eq!(error.kind(), ErrorKind::Decode);
}

#[tokio::test]
async fn list_ids_are_encoded_as_one_path_segment() {
    let graph = Graph::new().await;
    Mock::given(method("GET"))
        .and(path("/v1.0/me/todo/lists/a%2Fb%3Fc/tasks"))
        .respond_with(page(Vec::new(), None))
        .expect(1)
        .mount(&graph.server)
        .await;

    let items = graph.client.list_tasks("a/b?c").await.expect("tasks");

    assert!(items.is_empty());
}

#[tokio::test]
async fn raw_get_returns_the_body_and_sends_no_prefer() {
    let graph = Graph::new().await;
    Mock::given(method("GET"))
        .and(path("/v1.0/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "displayName": "BK" })))
        .expect(1)
        .mount(&graph.server)
        .await;

    let body = graph.client.get_raw("/me").await.expect("raw");

    assert_eq!(body["displayName"], "BK");
    let requests = graph.server.received_requests().await.expect("recorded");
    assert!(requests[0].headers.get("Prefer").is_none());
}

#[tokio::test]
async fn raw_get_only_reaches_paths_under_the_graph_root() {
    let graph = Graph::new().await;
    for bad in [
        "https://attacker.example/me",
        "me",
        "//attacker.example/me",
        "/../../common/oauth2/v2.0/token",
    ] {
        let error = graph.client.get_raw(bad).await.expect_err(bad);
        assert_eq!(error.kind(), ErrorKind::InvalidInput, "{bad}");
    }
    assert!(
        graph
            .server
            .received_requests()
            .await
            .expect("recorded")
            .is_empty()
    );
}

const EXTENSION: &str = "com.planetaryescape.mstodo";

#[tokio::test]
async fn lists_with_our_extension_send_the_filtered_expand_on_every_page() {
    let graph = Graph::new().await;
    Mock::given(method("GET"))
        .and(path("/v1.0/me/todo/lists"))
        .and(query_param(
            "$expand",
            "extensions($filter=id eq 'com.planetaryescape.mstodo')",
        ))
        .respond_with(page(
            vec![json!({ "id": "L1", "extensions": [{ "folder": "Home" }] })],
            None,
        ))
        .expect(1)
        .mount(&graph.server)
        .await;

    let lists = graph
        .client
        .list_lists_with_extension(EXTENSION)
        .await
        .expect("lists");

    assert_eq!(lists[0]["extensions"][0]["folder"], "Home");
}

/// Answers each `$batch` from `answer(sub-request id)`, and records the
/// requests each batch carried.
fn batch_responder(
    seen: std::sync::Arc<std::sync::Mutex<Vec<Value>>>,
    answer: impl Fn(&str, usize) -> (u16, Value) + Send + Sync + 'static,
) -> impl Fn(&wiremock::Request) -> ResponseTemplate + Send + Sync + 'static {
    move |request: &wiremock::Request| {
        let body: Value = serde_json::from_slice(&request.body).expect("json");
        let round = {
            let mut seen = seen.lock().expect("lock");
            seen.push(body.clone());
            seen.len()
        };
        let responses: Vec<Value> = body["requests"]
            .as_array()
            .expect("requests")
            .iter()
            .map(|sub| {
                let id = sub["id"].as_str().expect("id");
                let (status, body) = answer(id, round);
                json!({ "id": id, "status": status, "headers": {}, "body": body })
            })
            .collect();
        ResponseTemplate::new(200).set_body_json(json!({ "responses": responses }))
    }
}

#[tokio::test]
async fn a_batch_chains_its_gets_and_requeues_what_a_failed_step_blocked() {
    let graph = Graph::new().await;
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    Mock::given(method("POST"))
        .and(path("/v1.0/$batch"))
        .respond_with(batch_responder(seen.clone(), |id, round| {
            match (id, round) {
                ("1", _) => (
                    404,
                    json!({ "error": { "code": "ErrorItemNotFound", "message": "gone" } }),
                ),
                // After the 404 in round 1, the rest of the chain never ran.
                ("2", 1) => (
                    424,
                    json!({ "error": { "code": "FailedDependency", "message": "" } }),
                ),
                (id, _) => (200, json!({ "id": format!("T{id}") })),
            }
        }))
        .mount(&graph.server)
        .await;
    let urls: Vec<_> = ["T0", "T1", "T2"]
        .iter()
        .map(|task| graph.client.task_with_extension_url("L1", task, EXTENSION))
        .collect();

    let results = graph.client.get_each(&urls).await.expect("batch");

    assert_eq!(results[0].as_ref().expect("T0")["id"], "T0");
    assert_eq!(
        results[1].as_ref().expect_err("gone").kind(),
        ErrorKind::NotFound
    );
    assert_eq!(results[2].as_ref().expect("T2")["id"], "T2");
    let batches = seen.lock().expect("lock").clone();
    assert_eq!(batches.len(), 2);
    assert_eq!(
        batches[0]["requests"],
        json!([
            { "id": "0", "method": "GET", "url": "/me/todo/lists/L1/tasks/T0?$expand=extensions($filter=id%20eq%20%27com.planetaryescape.mstodo%27)" },
            { "id": "1", "method": "GET", "url": "/me/todo/lists/L1/tasks/T1?$expand=extensions($filter=id%20eq%20%27com.planetaryescape.mstodo%27)", "dependsOn": ["0"] },
            { "id": "2", "method": "GET", "url": "/me/todo/lists/L1/tasks/T2?$expand=extensions($filter=id%20eq%20%27com.planetaryescape.mstodo%27)", "dependsOn": ["1"] }
        ])
    );
    assert_eq!(
        batches[1]["requests"].as_array().expect("requests").len(),
        1
    );
    assert!(batches[1]["requests"][0].get("dependsOn").is_none());
}

#[tokio::test]
async fn a_batch_holds_at_most_20_requests_and_retries_a_throttled_one() {
    let graph = Graph::new().await;
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    Mock::given(method("POST"))
        .and(path("/v1.0/$batch"))
        .respond_with(batch_responder(seen.clone(), |id, round| {
            match (id, round) {
                ("0", 1) => (
                    429,
                    json!({ "error": { "code": "activityLimitReached", "message": "" } }),
                ),
                // The rest of the chain never ran.
                (_, 1) => (424, json!({})),
                (id, _) => (200, json!({ "id": id })),
            }
        }))
        .mount(&graph.server)
        .await;
    let urls: Vec<_> = (0..25)
        .map(|n| {
            graph
                .client
                .task_with_extension_url("L1", &format!("T{n}"), EXTENSION)
        })
        .collect();

    let results = graph.client.get_each(&urls).await.expect("batch");

    assert!(results.iter().all(Result::is_ok));
    let sizes: Vec<usize> = seen
        .lock()
        .expect("lock")
        .iter()
        .map(|batch| batch["requests"].as_array().expect("requests").len())
        .collect();
    // Round 1: 0 is throttled and 1..19 blocked; they go first in round 2,
    // ahead of 20..24.
    assert_eq!(sizes, [20, 20, 5]);
}
