//! Rung 2's writes through the real binary and daemon, against wiremock:
//! the requests Graph gets (bodies, `If-Match`, how many), what the CLI
//! prints, and its exit codes.

mod support;

use serde_json::{Value, json};
use support::{ACCESS_TOKEN, Env};
use wiremock::matchers::{body_json, body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const LIST: &str = "/v1.0/me/todo/lists/L-tasks/tasks";

/// Graph with a default "Tasks" list and a "Groceries" list, signed in.
async fn graph(env: &mut Env) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1.0/me/todo/lists"))
        .and(header("Authorization", format!("Bearer {ACCESS_TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "value": [
            { "id": "L-tasks", "displayName": "Tasks", "wellknownListName": "defaultList", "isOwner": true, "isShared": false },
            { "id": "L-groc", "displayName": "Groceries", "wellknownListName": "none", "isOwner": true, "isShared": false }
        ]})))
        .mount(&server)
        .await;
    env.graph_url = Some(format!("{}/v1.0", server.uri()));
    env.sign_in();
    server
}

fn task(id: &str, title: &str, etag: &str) -> Value {
    json!({
        "@odata.etag": etag,
        "id": id,
        "title": title,
        "status": "notStarted",
        "importance": "normal",
        "isReminderOn": false,
        "categories": [],
        "createdDateTime": "2026-09-24T10:00:00.1234567Z",
        "lastModifiedDateTime": "2026-09-24T10:00:00.1234567Z"
    })
}

async fn tasks_in_default_list(server: &MockServer, tasks: Vec<Value>) {
    Mock::given(method("GET"))
        .and(path(LIST))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "value": tasks })))
        .mount(server)
        .await;
}

/// Every request other than a GET: what reached Graph as a write.
async fn writes(server: &MockServer) -> Vec<Request> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|request| request.method.as_str() != "GET")
        .collect()
}

fn stderr_json(output: &std::process::Output) -> Value {
    serde_json::from_slice(&output.stderr).expect("json error on stderr")
}

#[tokio::test]
async fn add_posts_the_literal_title_with_its_op_id_in_our_extension() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    Mock::given(method("POST"))
        .and(path(LIST))
        .respond_with(|request: &Request| {
            let mut created: Value = serde_json::from_slice(&request.body).expect("json body");
            created["id"] = json!("T-new");
            created["@odata.etag"] = json!("W/\"e1\"");
            ResponseTemplate::new(201).set_body_json(created)
        })
        .mount(&graph)
        .await;

    let added = env.json(&[
        "tasks",
        "add",
        "Buy milk #Home !9am tomorrow",
        "--due",
        "2026-09-26",
        "--reminder",
        "2026-09-26T08:30",
        "--importance",
        "high",
        "--body",
        "2 pints",
    ]);

    assert_eq!(added["schema_version"], 1);
    assert_eq!(added["action"], "add");
    assert_eq!(added["items"][0]["id"], "T-new");
    assert_eq!(added["list_ids"], json!(["L-tasks"]));
    let op_id = added["op_id"].as_str().expect("op_id");

    let sent = writes(&graph).await;
    assert_eq!(sent.len(), 1);
    let body: Value = serde_json::from_slice(&sent[0].body).expect("json");
    assert_eq!(
        body,
        json!({
            // Taken literally: no parsing in rung 2.
            "title": "Buy milk #Home !9am tomorrow",
            "dueDateTime": { "dateTime": "2026-09-26T00:00:00", "timeZone": "Europe/London" },
            "isReminderOn": true,
            "reminderDateTime": { "dateTime": "2026-09-26T08:30:00", "timeZone": "Europe/London" },
            "importance": "high",
            "body": { "content": "2 pints", "contentType": "text" },
            "extensions": [{
                "@odata.type": "microsoft.graph.openTypeExtension",
                "extensionName": "com.planetaryescape.mstodo",
                "opId": op_id
            }]
        })
    );
}

#[tokio::test]
async fn a_create_that_gets_a_5xx_is_outcome_unknown_and_never_resent() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    Mock::given(method("POST"))
        .and(path(LIST))
        .respond_with(
            ResponseTemplate::new(503)
                .insert_header("request-id", "req-503")
                .set_body_json(
                    json!({ "error": { "code": "ServiceUnavailable", "message": "try later" } }),
                ),
        )
        .mount(&graph)
        .await;

    let failed = env
        .cmd()
        .args(["--format", "json", "tasks", "add", "Buy milk"])
        .assert()
        .code(1);

    let output = failed.get_output();
    assert!(output.stdout.is_empty());
    let error = stderr_json(output);
    assert_eq!(error["error"]["kind"], "outcome_unknown");
    assert_eq!(error["error"]["request_id"], "req-503");
    let op_id = error["error"]["op_id"].as_str().expect("op_id");
    let message = error["error"]["message"].as_str().expect("message");
    assert!(message.contains("ms-todo tasks list"), "{message}");
    assert!(message.contains(op_id), "{message}");

    let sent = writes(&graph).await;
    assert_eq!(sent.len(), 1, "a create is never resent after a 5xx");
    let body: Value = serde_json::from_slice(&sent[0].body).expect("json");
    assert_eq!(body["extensions"][0]["opId"], op_id);
}

#[tokio::test]
async fn a_create_that_gets_a_429_is_resent() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    Mock::given(method("POST"))
        .and(path(LIST))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "0"))
        .up_to_n_times(1)
        .mount(&graph)
        .await;
    Mock::given(method("POST"))
        .and(path(LIST))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(task("T-new", "Buy milk", "W/\"e1\"")),
        )
        .mount(&graph)
        .await;

    let added = env.json(&["tasks", "add", "Buy milk"]);

    assert_eq!(added["items"][0]["id"], "T-new");
    let sent = writes(&graph).await;
    assert_eq!(sent.len(), 2, "a 429 proves the create didn't run");
    assert_eq!(sent[0].body, sent[1].body, "the same opId both times");
}

#[tokio::test]
async fn complete_and_reopen_send_if_match_with_the_etag_last_read() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    tasks_in_default_list(&graph, vec![task("T1", "Buy milk", "W/\"e1\"")]).await;
    let mut done = task("T1", "Buy milk", "W/\"e2\"");
    done["status"] = json!("completed");
    Mock::given(method("PATCH"))
        .and(path(format!("{LIST}/T1")))
        .and(header("If-Match", "W/\"e1\""))
        .and(body_json(json!({ "status": "completed" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(done))
        .mount(&graph)
        .await;
    Mock::given(method("PATCH"))
        .and(path(format!("{LIST}/T1")))
        .and(header("If-Match", "W/\"e2\""))
        .and(body_json(json!({ "status": "notStarted" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(task("T1", "Buy milk", "W/\"e3\"")))
        .mount(&graph)
        .await;

    env.json(&["tasks", "list"]);
    let completed = env.json(&["tasks", "complete", "T1"]);
    assert_eq!(completed["action"], "complete");
    assert_eq!(completed["items"][0]["status"], "completed");
    assert!(completed["op_id"].as_str().is_some_and(|id| id.len() == 36));

    // The etag from the complete's response, not from the earlier list.
    let reopened = env.json(&["tasks", "reopen", "T1"]);
    assert_eq!(reopened["items"][0]["status"], "notStarted");
    assert_eq!(writes(&graph).await.len(), 2);
}

#[tokio::test]
async fn a_task_the_daemon_has_not_seen_is_found_by_asking_each_list() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    Mock::given(method("GET"))
        .and(path(format!("{LIST}/T9")))
        .respond_with(ResponseTemplate::new(404).set_body_json(
            json!({ "error": { "code": "ErrorItemNotFound", "message": "not here" } }),
        ))
        .mount(&graph)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1.0/me/todo/lists/L-groc/tasks/T9"))
        .respond_with(ResponseTemplate::new(200).set_body_json(task("T9", "Eggs", "W/\"g1\"")))
        .mount(&graph)
        .await;
    let mut done = task("T9", "Eggs", "W/\"g2\"");
    done["status"] = json!("completed");
    Mock::given(method("PATCH"))
        .and(path("/v1.0/me/todo/lists/L-groc/tasks/T9"))
        .and(header("If-Match", "W/\"g1\""))
        .respond_with(ResponseTemplate::new(200).set_body_json(done))
        .mount(&graph)
        .await;

    let completed = env.json(&["tasks", "complete", "T9"]);

    assert_eq!(completed["list_ids"], json!(["L-groc"]));
    let missing = env
        .cmd()
        .args(["--format", "json", "tasks", "complete", "T-nowhere"])
        .assert()
        .code(3);
    assert_eq!(
        stderr_json(missing.get_output())["error"]["kind"],
        "not_found"
    );
}

/// T1 as last read (e1), then as changed on the server (e2) by `server_change`.
async fn stale_etag_graph(env: &mut Env, server_change: Value) -> MockServer {
    let graph = graph(env).await;
    tasks_in_default_list(&graph, vec![task("T1", "Buy milk", "W/\"e1\"")]).await;
    Mock::given(method("PATCH"))
        .and(path(format!("{LIST}/T1")))
        .and(header("If-Match", "W/\"e1\""))
        .respond_with(ResponseTemplate::new(412).set_body_json(json!({ "error": {
            "code": "ErrorIrresolvableConflict",
            "message": "A precondition provided in the request (such as an if-match header) does not match the resource's current state."
        }})))
        .mount(&graph)
        .await;
    let mut current = task("T1", "Buy milk", "W/\"e2\"");
    if let (Value::Object(current), Value::Object(change)) = (&mut current, server_change) {
        current.extend(change);
    }
    Mock::given(method("GET"))
        .and(path(format!("{LIST}/T1")))
        .respond_with(ResponseTemplate::new(200).set_body_json(current))
        .mount(&graph)
        .await;
    let mut edited = task("T1", "Oat milk", "W/\"e3\"");
    edited["importance"] = json!("high");
    Mock::given(method("PATCH"))
        .and(path(format!("{LIST}/T1")))
        .and(header("If-Match", "W/\"e2\""))
        .respond_with(ResponseTemplate::new(200).set_body_json(edited))
        .mount(&graph)
        .await;
    env.json(&["tasks", "list"]);
    graph
}

#[tokio::test]
async fn a_412_on_a_field_nobody_else_touched_is_re_sent_with_the_new_etag() {
    let mut env = Env::new();
    // The phone changed the importance; we change the title.
    let graph = stale_etag_graph(&mut env, json!({ "importance": "high" })).await;

    let edited = env.json(&["tasks", "edit", "T1", "--title", "Oat milk"]);

    assert_eq!(edited["items"][0]["title"], "Oat milk");
    let sent = writes(&graph).await;
    assert_eq!(sent.len(), 2);
    for request in &sent {
        let body: Value = serde_json::from_slice(&request.body).expect("json");
        assert_eq!(
            body,
            json!({ "title": "Oat milk" }),
            "only the changed field"
        );
    }
}

#[tokio::test]
async fn a_412_on_a_field_the_server_changed_too_is_a_conflict() {
    let mut env = Env::new();
    let graph = stale_etag_graph(&mut env, json!({ "title": "Almond milk" })).await;

    let conflict = env
        .cmd()
        .args([
            "--format", "json", "tasks", "edit", "T1", "--title", "Oat milk",
        ])
        .assert()
        .code(5);

    let error = stderr_json(conflict.get_output());
    assert_eq!(error["error"]["kind"], "conflict");
    assert!(
        error["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("title")),
        "{error}"
    );
    assert_eq!(writes(&graph).await.len(), 1, "nothing overwritten");
}

#[tokio::test]
async fn delete_needs_yes_off_a_terminal_and_a_404_counts_as_deleted() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    tasks_in_default_list(&graph, vec![task("T1", "Buy milk", "W/\"e1\"")]).await;
    Mock::given(method("DELETE"))
        .and(path(format!("{LIST}/T1")))
        .respond_with(
            ResponseTemplate::new(404).set_body_json(
                json!({ "error": { "code": "ErrorItemNotFound", "message": "gone" } }),
            ),
        )
        .mount(&graph)
        .await;

    // Refused before any daemon starts; it never prompts.
    let refused = env
        .cmd()
        .args(["--format", "json", "tasks", "delete", "T1"])
        .assert()
        .code(2);
    let error = stderr_json(refused.get_output());
    assert!(
        error["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("--yes"))
    );
    assert_eq!(env.json(&["daemon", "status"])["running"], false);

    let deleted = env.json(&["tasks", "delete", "T1", "--list", "Tasks", "--yes"]);
    assert_eq!(deleted["action"], "delete");
    assert_eq!(deleted["items"][0]["id"], "T1");
    let sent = writes(&graph).await;
    assert_eq!(sent.len(), 1);
    assert!(
        sent[0].headers.get("If-Match").is_none(),
        "task DELETE ignores If-Match (S6)"
    );
}

#[tokio::test]
async fn a_dry_run_shows_the_plan_and_writes_nothing() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    tasks_in_default_list(
        &graph,
        vec![
            task("T1", "Buy milk", "W/\"e1\""),
            task("T2", "Call mum", "W/\"e2\""),
        ],
    )
    .await;

    let plan = env.json(&[
        "tasks",
        "delete",
        "Call mum",
        "T1",
        "--list",
        "Tasks",
        "--dry-run",
    ]);
    assert_eq!(plan["dry_run"], true);
    assert_eq!(plan["action"], "delete");
    assert_eq!(
        plan["targets"],
        json!([
            { "id": "T2", "title": "Call mum", "list_id": "L-tasks" },
            { "id": "T1", "title": "Buy milk", "list_id": "L-tasks" }
        ])
    );

    let add = env.json(&[
        "tasks",
        "add",
        "Buy bread",
        "--due",
        "2026-09-26",
        "--dry-run",
    ]);
    assert_eq!(add["list"]["id"], "L-tasks");
    assert_eq!(add["changes"]["title"], "Buy bread");
    assert!(add["changes"].get("extensions").is_none());

    let edit = env.json(&["tasks", "edit", "T1", "--clear-due", "--dry-run"]);
    assert_eq!(edit["changes"], json!({ "dueDateTime": null }));

    assert!(writes(&graph).await.is_empty(), "a dry run writes nothing");
}

#[tokio::test]
async fn ids_on_stdin_are_completed_in_order() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    tasks_in_default_list(
        &graph,
        vec![
            task("T1", "Buy milk", "W/\"e1\""),
            task("T2", "Call mum", "W/\"e2\""),
        ],
    )
    .await;
    Mock::given(method("PATCH"))
        .and(body_partial_json(json!({ "status": "completed" })))
        .respond_with(|request: &Request| {
            let id = request.url.path().rsplit('/').next().unwrap_or_default();
            let mut done = task(id, "done", "W/\"x\"");
            done["status"] = json!("completed");
            ResponseTemplate::new(200).set_body_json(done)
        })
        .mount(&graph)
        .await;

    let ids = env
        .cmd()
        .args(["--format", "ids", "tasks", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let completed = env
        .cmd()
        .args(["--format", "json", "tasks", "complete", "-"])
        .write_stdin(ids)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let completed: Value = serde_json::from_slice(&completed).expect("json");
    let ids: Vec<&str> = completed["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter_map(|task| task["id"].as_str())
        .collect();
    assert_eq!(ids, ["T1", "T2"]);
    let paths: Vec<String> = writes(&graph)
        .await
        .iter()
        .map(|request| request.url.path().to_owned())
        .collect();
    assert_eq!(paths, [format!("{LIST}/T1"), format!("{LIST}/T2")]);
}

#[tokio::test]
async fn a_title_two_tasks_share_exits_2_with_the_candidates() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    tasks_in_default_list(
        &graph,
        vec![
            task("T1", "Call mum", "W/\"e1\""),
            task("T2", "Call mum", "W/\"e2\""),
        ],
    )
    .await;

    let ambiguous = env
        .cmd()
        .args([
            "--format", "json", "tasks", "complete", "Call mum", "--list", "Tasks",
        ])
        .assert()
        .code(2);

    let error = stderr_json(ambiguous.get_output());
    assert_eq!(error["error"]["kind"], "invalid_input");
    assert_eq!(
        error["error"]["candidates"],
        json!([{ "id": "T1", "name": "Call mum" }, { "id": "T2", "name": "Call mum" }])
    );
    assert!(writes(&graph).await.is_empty());
}

fn recurring(etag: &str, due: &str) -> Value {
    let mut task = task("T-r", "Water plants", etag);
    task["recurrence"] = json!({ "pattern": { "type": "weekly", "interval": 1 } });
    task["dueDateTime"] = json!({ "dateTime": due, "timeZone": "UTC" });
    task
}

#[tokio::test]
async fn completing_a_recurring_task_reports_the_rolled_due_date() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    tasks_in_default_list(
        &graph,
        vec![recurring("W/\"r1\"", "2026-09-24T00:00:00.0000000")],
    )
    .await;
    // Each completion moves the due date on a week (S12).
    Mock::given(method("PATCH"))
        .and(path(format!("{LIST}/T-r")))
        .and(header("If-Match", "W/\"r1\""))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(recurring("W/\"r2\"", "2026-10-01T00:00:00.0000000")),
        )
        .mount(&graph)
        .await;
    Mock::given(method("PATCH"))
        .and(path(format!("{LIST}/T-r")))
        .and(header("If-Match", "W/\"r2\""))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(recurring("W/\"r3\"", "2026-10-08T00:00:00.0000000")),
        )
        .mount(&graph)
        .await;
    env.json(&["tasks", "list"]);

    let completed = env.json(&["tasks", "complete", "T-r"]);
    assert_eq!(
        completed["rolled"],
        json!([{ "id": "T-r", "next_due": "2026-10-01" }])
    );

    let table = env
        .cmd()
        .args(["--format", "table", "tasks", "complete", "T-r"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let table = String::from_utf8(table).expect("utf8");
    assert!(table.contains("now due 2026-10-08"), "{table}");
    assert!(table.contains("new, completed task"), "{table}");
}

#[tokio::test]
async fn completing_a_recurring_task_is_never_resent_after_a_5xx() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    tasks_in_default_list(
        &graph,
        vec![recurring("W/\"r1\"", "2026-09-24T00:00:00.0000000")],
    )
    .await;
    Mock::given(method("PATCH"))
        .and(path(format!("{LIST}/T-r")))
        .respond_with(ResponseTemplate::new(500))
        .mount(&graph)
        .await;
    env.json(&["tasks", "list"]);

    let unknown = env
        .cmd()
        .args(["--format", "json", "tasks", "complete", "T-r"])
        .assert()
        .code(1);

    let error = stderr_json(unknown.get_output());
    assert_eq!(error["error"]["kind"], "outcome_unknown");
    assert!(error["error"]["op_id"].is_string());
    assert_eq!(writes(&graph).await.len(), 1);
}

#[tokio::test]
async fn raw_writes_need_yes_off_a_terminal_and_are_never_resent() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    Mock::given(method("PATCH"))
        .and(path("/v1.0/me/todo/lists/L-tasks"))
        .respond_with(ResponseTemplate::new(502))
        .mount(&graph)
        .await;

    let body = r#"{"displayName":"Renamed"}"#;
    env.cmd()
        .args(["raw", "PATCH", "/me/todo/lists/L-tasks", "--body", body])
        .assert()
        .code(2);
    assert!(writes(&graph).await.is_empty());

    let unknown = env
        .cmd()
        .args([
            "--format",
            "json",
            "raw",
            "PATCH",
            "/me/todo/lists/L-tasks",
            "--body",
            body,
            "--yes",
        ])
        .assert()
        .code(1);
    let error = stderr_json(unknown.get_output());
    assert_eq!(error["error"]["kind"], "outcome_unknown");
    assert!(
        error["error"]["op_id"].is_string(),
        "the op_id the CLI sent"
    );
    let sent = writes(&graph).await;
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].body, body.as_bytes());
}

#[tokio::test]
async fn tasks_list_as_csv_has_fixed_columns_and_quotes_what_needs_it() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    let mut tricky = task("T2", "Milk, eggs \"large\"\nand bread", "W/\"e2\"");
    tricky["categories"] = json!(["Home", "Shop"]);
    tricky["importance"] = json!("high");
    tricky["isReminderOn"] = json!(true);
    tricky["reminderDateTime"] =
        json!({ "dateTime": "2026-09-26T16:30:00.0000000", "timeZone": "UTC" });
    // Midnight in London during BST is 23:00 UTC the day before (S11).
    tricky["dueDateTime"] = json!({ "dateTime": "2026-09-25T23:00:00.0000000", "timeZone": "UTC" });
    tricky["body"] = json!({ "content": "not a column", "contentType": "text" });
    tasks_in_default_list(&graph, vec![task("T1", "Buy milk", "W/\"e1\""), tricky]).await;

    let csv = env
        .cmd()
        .args(["--format", "csv", "tasks", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    insta::assert_snapshot!(String::from_utf8(csv).expect("utf8"));
}
