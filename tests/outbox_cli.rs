//! Rung 4's outbox through the real binary and daemon, against a fake
//! Graph (docs/blueprint/04-sync-cache.md#instant-local-writes): writes
//! answer at once and work offline, go out in order, and are never lost:
//! a rejection rolls back and stays `failed`, an ambiguous outcome stays
//! `unknown` until it's attributed by `opId`, and `undo` reverses a change.

mod support;

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use support::Env;
use support::fake_graph::{FakeGraph, list, task};
use wiremock::matchers::{body_json, body_partial_json, header, method, path};
use wiremock::{Mock, Request, ResponseTemplate};

const TASKS: &str = "/v1.0/me/todo/lists/L-tasks/tasks";

/// Graph with "Tasks" holding `tasks` and an empty "Groceries".
async fn graph_with(env: &mut Env, tasks: Vec<Value>) -> FakeGraph {
    let graph = FakeGraph::start(
        env,
        vec![
            list("L-tasks", "Tasks", "defaultList"),
            list("L-groc", "Groceries", "none"),
        ],
    )
    .await;
    graph.edit(|data| {
        data.tasks.insert("L-tasks".into(), tasks);
        data.tasks.insert("L-groc".into(), Vec::new());
    });
    graph
}

/// A POST answered with the task it asked for, as `id`.
fn created_as(id: &'static str) -> impl Fn(&Request) -> ResponseTemplate {
    move |request: &Request| {
        let mut created: Value = serde_json::from_slice(&request.body).expect("json body");
        created["id"] = json!(id);
        created["@odata.etag"] = json!(format!("W/\"{id}-1\""));
        created["status"] = created
            .get("status")
            .cloned()
            .unwrap_or(json!("notStarted"));
        ResponseTemplate::new(201).set_body_json(created)
    }
}

/// A PATCH answered with `base` changed by what was sent, and a new etag.
fn patched(base: Value, etag: &'static str) -> impl Fn(&Request) -> ResponseTemplate {
    move |request: &Request| {
        let sent: Value = serde_json::from_slice(&request.body).expect("json body");
        let mut task = base.clone();
        if let (Some(task), Some(sent)) = (task.as_object_mut(), sent.as_object()) {
            task.extend(sent.clone());
        }
        task["@odata.etag"] = json!(etag);
        ResponseTemplate::new(200).set_body_json(task)
    }
}

/// Throttled with a wait longer than a request may sleep: a temporary
/// failure the outbox backs off from, so the write stays `pending`.
fn throttled() -> ResponseTemplate {
    ResponseTemplate::new(429)
        .insert_header("Retry-After", "120")
        .set_body_json(json!({ "error": { "code": "TooManyRequests", "message": "slow down" } }))
}

fn op_id(applied: &Value) -> String {
    applied["op_id"].as_str().expect("op_id").to_owned()
}

fn tasks(env: &Env, list: &str) -> Vec<Value> {
    env.json(&["tasks", "list", "--list", list])["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

async fn writes_to(graph: &FakeGraph, verb: &str) -> Vec<Request> {
    graph
        .writes()
        .await
        .into_iter()
        .filter(|request| request.method.as_str() == verb)
        .collect()
}

#[tokio::test]
async fn an_offline_add_answers_at_once_as_pending_and_syncs_when_graph_is_back() {
    let mut env = Env::new();
    let graph = graph_with(&mut env, Vec::new()).await;
    Mock::given(method("POST"))
        .and(path(TASKS))
        .respond_with(created_as("T-new"))
        .mount(&graph.server)
        .await;
    env.synced();
    env.json(&["daemon", "stop"]);

    // The network goes: nothing listens where Graph was.
    let online = env.graph_url.replace("http://127.0.0.1:9/v1.0".into());
    env.json(&["daemon", "start"]);
    let started = Instant::now();
    let added = env.json(&["tasks", "add", "Buy milk"]);
    let took = started.elapsed();
    eprintln!("offline `tasks add` answered in {took:?}");
    assert!(took < Duration::from_secs(2), "took {took:?}");
    assert_eq!(added["items"][0]["sync_state"], "pending");
    assert_eq!(added["items"][0]["graph_id"], Value::Null);
    let listed = tasks(&env, "Tasks");
    assert_eq!(listed[0]["title"], "Buy milk");
    assert_eq!(listed[0]["sync_state"], "pending");

    // It was tried, found no network, and waits.
    let op = op_id(&added);
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let found = env.op_in_state(&op, "pending");
        if found["last_error"]["kind"] == "network" {
            break;
        }
        assert!(Instant::now() < deadline, "never tried: {found}");
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(graph.writes().await.is_empty());

    // The network is back.
    env.json(&["daemon", "stop"]);
    env.graph_url = online;
    env.settled();
    let listed = tasks(&env, "Tasks");
    assert_eq!(listed[0]["id"], added["items"][0]["id"]);
    assert_eq!(listed[0]["graph_id"], "T-new");
    assert_eq!(listed[0]["sync_state"], "synced");
    assert_eq!(env.op_in_state(&op, "done")["attempts"], 2);
    assert_eq!(writes_to(&graph, "POST").await.len(), 1);
}

#[tokio::test]
async fn an_edit_of_a_task_not_created_yet_waits_for_its_create_and_uses_its_etag() {
    let mut env = Env::new();
    let graph = graph_with(&mut env, Vec::new()).await;
    Mock::given(method("POST"))
        .and(path(TASKS))
        .respond_with(move |request: &Request| {
            created_as("T-new")(request).set_delay(Duration::from_millis(800))
        })
        .mount(&graph.server)
        .await;
    Mock::given(method("PATCH"))
        .and(path(format!("{TASKS}/T-new")))
        .and(header("If-Match", "W/\"T-new-1\""))
        .and(body_json(json!({ "title": "Buy oat milk" })))
        .respond_with(patched(task("T-new", "Buy milk", "x"), "W/\"T-new-2\""))
        .mount(&graph.server)
        .await;
    env.synced();

    let added = env.json(&["tasks", "add", "Buy milk"]);
    let local = added["items"][0]["id"].as_str().expect("id").to_owned();
    let edited = env.json(&["tasks", "edit", &local, "--title", "Buy oat milk"]);

    assert_eq!(edited["items"][0]["title"], "Buy oat milk");
    let edit = env
        .outbox()
        .into_iter()
        .find(|op| op["op_id"] == edited["op_id"])
        .expect("the edit is queued");
    assert_eq!(edit["depends_on"], added["op_id"]);
    env.settled();
    let sent: Vec<String> = graph
        .writes()
        .await
        .iter()
        .map(|request| format!("{} {}", request.method, request.url.path()))
        .collect();
    assert_eq!(
        sent,
        [format!("POST {TASKS}"), format!("PATCH {TASKS}/T-new")]
    );
    let listed = tasks(&env, "Tasks");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["id"], local.as_str());
    assert_eq!(listed[0]["title"], "Buy oat milk");
    assert_eq!(listed[0]["@odata.etag"], "W/\"T-new-2\"");
}

#[tokio::test]
async fn a_permanent_rejection_rolls_back_and_is_kept_as_failed() {
    let mut env = Env::new();
    let graph = graph_with(&mut env, vec![task("T1", "Buy milk", "W/\"e1\"")]).await;
    Mock::given(method("PATCH"))
        .and(path(format!("{TASKS}/T1")))
        .respond_with(ResponseTemplate::new(400).set_body_json(
            json!({ "error": { "code": "invalidRequest", "message": "Invalid request." } }),
        ))
        .mount(&graph.server)
        .await;
    env.synced();

    let edited = env.json(&["tasks", "edit", "T1", "--title", "Buy oat milk"]);
    assert_eq!(edited["items"][0]["title"], "Buy oat milk");

    let op = env.op_in_state(&op_id(&edited), "failed");
    assert_eq!(op["last_error"]["kind"], "rejected");
    assert_eq!(op["changes"], json!({ "title": "Buy oat milk" }));
    let listed = tasks(&env, "Tasks");
    assert_eq!(listed[0]["title"], "Buy milk", "rolled back");
    assert_eq!(listed[0]["sync_state"], "failed");
    assert_eq!(graph.writes().await.len(), 1, "a rejection isn't resent");
    let doctor = env.json(&["doctor"]);
    assert_eq!(doctor["outbox"]["failed"], 1);
}

#[tokio::test]
async fn a_create_into_a_list_deleted_meanwhile_is_rejected_by_the_ghost_write_check() {
    let mut env = Env::new();
    let graph = graph_with(&mut env, Vec::new()).await;
    Mock::given(method("POST"))
        .and(path(TASKS))
        .respond_with(created_as("T-ghost"))
        .mount(&graph.server)
        .await;
    env.synced();
    // S4: the POST still answers 201, but the list's own GET is a 404.
    graph.edit(|data| {
        data.gone_lists.insert("L-tasks".into());
    });

    let added = env.json(&["tasks", "add", "Buy milk"]);

    let op = env.op_in_state(&op_id(&added), "failed");
    assert_eq!(op["last_error"]["kind"], "rejected");
    assert!(
        op["last_error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("list was deleted")),
        "{op}"
    );
    assert_eq!(op["changes"]["title"], "Buy milk", "the content is kept");
    assert!(tasks(&env, "Tasks").is_empty());
    let lists_gets = graph
        .requests("GET")
        .await
        .into_iter()
        .filter(|request| request.url.path() == "/v1.0/me/todo/lists/L-tasks")
        .count();
    assert_eq!(lists_gets, 1, "one check, after the 201");
}

#[tokio::test]
async fn queued_writes_for_a_list_deleted_on_another_device_fail_and_keep_their_content() {
    let mut env = Env::new();
    let graph = graph_with(&mut env, Vec::new()).await;
    Mock::given(method("POST"))
        .and(path("/v1.0/me/todo/lists/L-groc/tasks"))
        .respond_with(throttled())
        .mount(&graph.server)
        .await;
    env.synced();

    let added = env.json(&["tasks", "add", "Eggs", "--list", "Groceries"]);
    let op = op_id(&added);
    let deadline = Instant::now() + Duration::from_secs(20);
    while env.op_in_state(&op, "pending")["last_error"]["kind"] != "rate_limited" {
        assert!(Instant::now() < deadline, "never tried");
        std::thread::sleep(Duration::from_millis(50));
    }
    graph.edit(|data| data.lists.retain(|list| list["id"] != "L-groc"));
    env.synced();

    let failed = env.op_in_state(&op, "failed");
    assert!(
        failed["last_error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("list was deleted")),
        "{failed}"
    );
    assert_eq!(failed["changes"]["title"], "Eggs");
}

#[tokio::test]
async fn an_unknown_create_is_adopted_only_by_its_op_id_in_one_merge() {
    let mut env = Env::new();
    let graph = graph_with(&mut env, Vec::new()).await;
    Mock::given(method("POST"))
        .and(path(TASKS))
        .respond_with(ResponseTemplate::new(503))
        .mount(&graph.server)
        .await;
    env.synced();

    let added = env.json(&["tasks", "add", "Buy milk"]);
    let op = op_id(&added);
    let local = added["items"][0]["id"].clone();
    env.op_in_state(&op, "unknown");

    // A task with the same title but no opId is someone else's.
    graph.edit(|data| {
        data.tasks.insert(
            "L-tasks".into(),
            vec![task("T-other", "Buy milk", "W/\"o\"")],
        );
    });
    env.synced();
    assert_eq!(env.op_in_state(&op, "unknown")["state"], "unknown");

    // Graph did make it: it carries our opId.
    graph.edit(|data| {
        data.tasks
            .get_mut("L-tasks")
            .expect("list")
            .push(task("T-ours", "Buy milk", "W/\"m\""));
        data.extensions.insert(
            "T-ours".into(),
            json!({
                "id": "microsoft.graph.openTypeExtension.com.planetaryescape.mstodo",
                "extensionName": "com.planetaryescape.mstodo",
                "opId": op
            }),
        );
    });
    env.synced();

    env.op_in_state(&op, "done");
    let listed = tasks(&env, "Tasks");
    let ours: Vec<&Value> = listed
        .iter()
        .filter(|task| task["graph_id"] == "T-ours")
        .collect();
    assert_eq!(ours.len(), 1, "one row: {listed:?}");
    assert_eq!(ours[0]["id"], local, "it keeps its local ID");
    assert_eq!(ours[0]["sync_state"], "synced");
    assert_eq!(listed.len(), 2, "T-other is untouched");
    assert_eq!(writes_to(&graph, "POST").await.len(), 1, "never resent");
}

#[tokio::test]
async fn writes_being_sent_when_the_daemon_stops_are_unknown_for_creates_and_resent_otherwise() {
    let mut env = Env::new();
    let graph = graph_with(&mut env, vec![task("T1", "Buy milk", "W/\"e1\"")]).await;
    let slow = Duration::from_secs(30);
    Mock::given(method("PATCH"))
        .and(path(format!("{TASKS}/T1")))
        .respond_with(ResponseTemplate::new(200).set_delay(slow))
        .up_to_n_times(1)
        .mount(&graph.server)
        .await;
    Mock::given(method("PATCH"))
        .and(path(format!("{TASKS}/T1")))
        .respond_with(patched(task("T1", "Buy milk", "x"), "W/\"e2\""))
        .mount(&graph.server)
        .await;
    Mock::given(method("POST"))
        .and(path(TASKS))
        .respond_with(ResponseTemplate::new(201).set_delay(slow))
        .mount(&graph.server)
        .await;
    env.synced();

    // A PATCH is safe to send again.
    let edited = env.json(&["tasks", "edit", "T1", "--title", "Buy oat milk"]);
    env.op_in_state(&op_id(&edited), "inflight");
    env.json(&["daemon", "stop"]);
    env.settled();
    assert_eq!(env.op_in_state(&op_id(&edited), "done")["attempts"], 2);
    assert_eq!(tasks(&env, "Tasks")[0]["title"], "Buy oat milk");

    // A create may have reached Graph.
    let added = env.json(&["tasks", "add", "Eggs"]);
    env.op_in_state(&op_id(&added), "inflight");
    env.json(&["daemon", "stop"]);
    let op = env.op_in_state(&op_id(&added), "unknown");
    assert!(
        op["note"]
            .as_str()
            .is_some_and(|note| note.contains("daemon stopped")),
        "{op}"
    );
    env.synced();
    assert_eq!(writes_to(&graph, "POST").await.len(), 1, "never resent");
}

#[tokio::test]
async fn sync_never_overwrites_or_removes_a_task_with_a_pending_write() {
    let mut env = Env::new();
    let graph = graph_with(&mut env, vec![task("T1", "Buy milk", "W/\"e1\"")]).await;
    Mock::given(method("PATCH"))
        .and(path(format!("{TASKS}/T1")))
        .respond_with(throttled())
        .mount(&graph.server)
        .await;
    env.synced();

    let edited = env.json(&["tasks", "edit", "T1", "--title", "Buy oat milk"]);
    let op = op_id(&edited);
    let deadline = Instant::now() + Duration::from_secs(20);
    while env.op_in_state(&op, "pending")["last_error"]["kind"] != "rate_limited" {
        assert!(Instant::now() < deadline, "never tried");
        std::thread::sleep(Duration::from_millis(50));
    }

    // The phone renames it: delta reports it.
    graph.edit(|data| {
        data.tasks.insert(
            "L-tasks".into(),
            vec![task("T1", "Phone title", "W/\"p1\"")],
        );
    });
    env.synced();
    let listed = tasks(&env, "Tasks");
    assert_eq!(listed[0]["title"], "Buy oat milk");
    assert_eq!(listed[0]["sync_state"], "pending");

    // The phone deletes it, and a whole read (after a lost delta token)
    // doesn't see it either.
    graph.edit(|data| {
        data.tasks.insert("L-tasks".into(), Vec::new());
    });
    env.synced();
    graph.edit(|data| data.forget_delta_tokens());
    env.synced();
    let listed = tasks(&env, "Tasks");
    assert_eq!(listed.len(), 1, "not tombstoned");
    assert_eq!(listed[0]["title"], "Buy oat milk");
}

#[tokio::test]
async fn undo_reverses_an_add_an_edit_a_complete_and_a_delete() {
    let mut env = Env::new();
    let graph = graph_with(&mut env, vec![task("T1", "Buy milk", "W/\"e1\"")]).await;
    Mock::given(method("POST"))
        .and(path(TASKS))
        .and(body_partial_json(json!({ "title": "Eggs" })))
        .respond_with(created_as("T-eggs"))
        .mount(&graph.server)
        .await;
    Mock::given(method("POST"))
        .and(path(TASKS))
        .and(body_partial_json(json!({ "title": "Buy milk" })))
        .respond_with(created_as("T1-again"))
        .mount(&graph.server)
        .await;
    Mock::given(method("DELETE"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&graph.server)
        .await;
    Mock::given(method("PATCH"))
        .and(path(format!("{TASKS}/T1")))
        .respond_with(patched(task("T1", "Buy milk", "x"), "W/\"e2\""))
        .mount(&graph.server)
        .await;
    env.synced();
    let t1 = env.local_id(&["tasks", "list"], "T1");

    // An add: undone by deleting what it made.
    let added = env.json(&["tasks", "add", "Eggs"]);
    env.settled();
    let first_undo = env.json(&["undo"]);
    assert_eq!(first_undo["action"], "undo");
    assert_eq!(first_undo["undoes"], added["op_id"]);
    env.settled();
    let deletes = writes_to(&graph, "DELETE").await;
    assert_eq!(deletes[0].url.path(), format!("{TASKS}/T-eggs"));
    assert_eq!(tasks(&env, "Tasks").len(), 1);

    // An edit: undone by putting back only what it changed.
    let edited = env.json(&["tasks", "edit", &t1, "--title", "Buy oat milk"]);
    env.settled();
    env.json(&["undo", &op_id(&edited)]);
    env.settled();
    let patches = writes_to(&graph, "PATCH").await;
    let last: Value = serde_json::from_slice(&patches[1].body).expect("json");
    assert_eq!(last, json!({ "title": "Buy milk" }));
    assert_eq!(tasks(&env, "Tasks")[0]["title"], "Buy milk");

    // A complete: undone by reopening.
    env.json(&["tasks", "complete", &t1]);
    env.settled();
    env.json(&["undo"]);
    env.settled();
    let patches = writes_to(&graph, "PATCH").await;
    let last: Value = serde_json::from_slice(&patches[3].body).expect("json");
    assert_eq!(last, json!({ "status": "notStarted" }));
    assert_eq!(tasks(&env, "Tasks")[0]["status"], "notStarted");

    // A delete: undone by creating it again, with a new Graph ID.
    env.json(&["tasks", "delete", &t1, "--yes"]);
    env.settled();
    assert!(tasks(&env, "Tasks").is_empty());
    let undone = env.json(&["undo"]);
    assert_eq!(undone["items"][0]["id"], t1.as_str());
    env.settled();
    let listed = tasks(&env, "Tasks");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["id"], t1.as_str(), "it keeps its local ID");
    assert_eq!(listed[0]["graph_id"], "T1-again");
    let posts = writes_to(&graph, "POST").await;
    let recreated: Value = serde_json::from_slice(&posts[1].body).expect("json");
    assert_eq!(recreated["title"], "Buy milk");
    assert_eq!(
        recreated["extensions"][0]["opId"], undone["op_id"],
        "the re-create carries the undo's own opId"
    );

    // Undoing an undo redoes: the add undone first comes back.
    let redo_target = first_undo["op_id"].as_str().expect("op_id").to_owned();
    env.json(&["undo", &redo_target]);
    env.settled();
    let titles: Vec<Value> = tasks(&env, "Tasks")
        .iter()
        .map(|task| task["title"].clone())
        .collect();
    assert!(titles.contains(&json!("Eggs")), "{titles:?}");

    // Nothing is left to undo twice.
    let again = env.failure(&["undo", &op_id(&edited)], 2);
    assert!(
        again["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("undone already")),
        "{again}"
    );
}

fn recurring(id: &str, etag: &str, due: &str) -> Value {
    let mut task = task(id, "Water plants", etag);
    task["recurrence"] = json!({ "pattern": { "type": "weekly", "interval": 1 } });
    task["dueDateTime"] = json!({ "dateTime": due, "timeZone": "UTC" });
    task
}

#[tokio::test]
async fn undoing_a_recurring_completion_needs_the_copy_named() {
    let mut env = Env::new();
    let graph = graph_with(
        &mut env,
        vec![recurring("T-r", "W/\"r1\"", "2026-09-24T00:00:00.0000000")],
    )
    .await;
    Mock::given(method("PATCH"))
        .and(path(format!("{TASKS}/T-r")))
        .and(body_json(json!({ "status": "completed" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(recurring(
            "T-r",
            "W/\"r2\"",
            "2026-10-01T00:00:00.0000000",
        )))
        .mount(&graph.server)
        .await;
    Mock::given(method("PATCH"))
        .and(path(format!("{TASKS}/T-r")))
        .and(body_partial_json(json!({ "dueDateTime": {} })))
        .respond_with(ResponseTemplate::new(200).set_body_json(recurring(
            "T-r",
            "W/\"r3\"",
            "2026-09-24T00:00:00.0000000",
        )))
        .mount(&graph.server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(format!("{TASKS}/T-copy")))
        .respond_with(ResponseTemplate::new(204))
        .mount(&graph.server)
        .await;
    env.synced();

    let completed = env.json(&["tasks", "complete", "T-r"]);
    env.settled();
    // "Can't undo yet": Graph's completed copy hasn't synced.
    let early = env.failure(&["undo"], 2);
    assert!(
        early["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("can't undo yet")),
        "{early}"
    );
    // S12: Graph made a completed copy with a new ID.
    graph.edit(|data| {
        let mut copy = task("T-copy", "Water plants", "W/\"c1\"");
        copy["status"] = json!("completed");
        copy["createdDateTime"] = json!(
            chrono::Utc::now()
                .format("%Y-%m-%dT%H:%M:%S%.6fZ")
                .to_string()
        );
        data.tasks.get_mut("L-tasks").expect("list").push(copy);
    });
    env.synced();
    let copy = env.local_id(&["tasks", "list"], "T-copy");

    let error = env.failure(&["undo"], 2);
    assert_eq!(error["error"]["kind"], "invalid_input");
    let candidates = error["error"]["candidates"].as_array().expect("candidates");
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0]["id"], copy.as_str());
    assert!(candidates[0]["created_at"].is_string());
    assert_eq!(writes_to(&graph, "DELETE").await.len(), 0);

    let undone = env.json(&["undo", &op_id(&completed), "--copy", &copy]);
    assert_eq!(undone["undoes"], completed["op_id"]);
    env.settled();
    let deletes = writes_to(&graph, "DELETE").await;
    assert_eq!(deletes.len(), 1);
    assert_eq!(deletes[0].url.path(), format!("{TASKS}/T-copy"));
    let patches = writes_to(&graph, "PATCH").await;
    let restore: Value = serde_json::from_slice(&patches[1].body).expect("json");
    assert_eq!(
        restore,
        json!({ "dueDateTime": { "dateTime": "2026-09-24T00:00:00.0000000", "timeZone": "UTC" } })
    );
}

#[tokio::test]
async fn outbox_retry_resends_an_unknown_write_and_discard_drops_one() {
    let mut env = Env::new();
    let graph = graph_with(&mut env, Vec::new()).await;
    Mock::given(method("POST"))
        .and(path(TASKS))
        .and(body_partial_json(json!({ "title": "Buy milk" })))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .mount(&graph.server)
        .await;
    Mock::given(method("POST"))
        .and(path(TASKS))
        .and(body_partial_json(json!({ "title": "Buy milk" })))
        .respond_with(created_as("T-milk"))
        .mount(&graph.server)
        .await;
    Mock::given(method("POST"))
        .and(path(TASKS))
        .and(body_partial_json(json!({ "title": "Eggs" })))
        .respond_with(ResponseTemplate::new(503))
        .mount(&graph.server)
        .await;
    env.synced();

    let milk = env.json(&["tasks", "add", "Buy milk"]);
    let eggs = env.json(&["tasks", "add", "Eggs"]);
    env.op_in_state(&op_id(&milk), "unknown");
    env.op_in_state(&op_id(&eggs), "unknown");
    let unknown = env.json(&["outbox", "list", "--state", "unknown"])["items"].clone();
    assert_eq!(unknown.as_array().map(Vec::len), Some(2));
    assert_eq!(env.json(&["doctor"])["outbox"]["unknown"], 2);

    // The user chose to resend it.
    let retried = env.json(&["outbox", "retry", &op_id(&milk)]);
    assert_eq!(retried["items"][0]["op_id"], milk["op_id"]);
    env.op_in_state(&op_id(&milk), "done");
    assert_eq!(tasks(&env, "Tasks").len(), 2);

    // Dropping one needs --yes off a terminal.
    env.failure(&["outbox", "discard", &op_id(&eggs)], 2);
    let discarded = env.json(&["outbox", "discard", &op_id(&eggs), "--yes"]);
    assert_eq!(discarded["items"][0]["state"], "unknown");
    assert!(env.outbox().iter().all(|op| op["op_id"] != eggs["op_id"]));
    let listed = tasks(&env, "Tasks");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["title"], "Buy milk");
    // A done write can't be retried or discarded, only undone.
    let done = env.failure(&["outbox", "discard", &op_id(&milk), "--yes"], 2);
    assert!(
        done["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("ms-todo undo")),
        "{done}"
    );
    let posts = writes_to(&graph, "POST").await;
    assert_eq!(posts.len(), 3, "milk twice (the user's retry), eggs once");
}

#[tokio::test]
async fn a_database_from_a_newer_ms_todo_is_refused_plainly() {
    let mut env = Env::new();
    let _graph = graph_with(&mut env, Vec::new()).await;
    env.synced();
    let database = env.json(&["doctor"])["database"]["path"]
        .as_str()
        .expect("database path")
        .to_owned();
    env.json(&["daemon", "stop"]);
    let mut connection =
        <sqlx::SqliteConnection as sqlx::Connection>::connect(&format!("sqlite:{database}"))
            .await
            .expect("open the database");
    sqlx::query(
        "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) \
         VALUES (9999, 'from a newer ms-todo', 1, x'00', 0)",
    )
    .execute(&mut connection)
    .await
    .expect("a newer migration");

    let error = env.failure(&["tasks", "list"], 1);

    assert_eq!(error["error"]["kind"], "database_too_new", "{error}");
    let message = error["error"]["message"].as_str().expect("message");
    assert!(message.contains("upgraded by a newer ms-todo"), "{message}");
    assert!(message.contains("install the latest version"), "{message}");
    assert!(!message.contains("migration"), "no sqlx detail: {message}");
}

#[tokio::test]
async fn undoing_a_delete_graph_then_rejects_never_makes_a_second_copy() {
    let mut env = Env::new();
    let graph = graph_with(&mut env, vec![task("T1", "Buy milk", "W/\"e1\"")]).await;
    Mock::given(method("DELETE"))
        .and(path(format!("{TASKS}/T1")))
        .respond_with(
            ResponseTemplate::new(403)
                .set_delay(Duration::from_millis(800))
                .set_body_json(json!({ "error": { "code": "accessDenied", "message": "no" } })),
        )
        .mount(&graph.server)
        .await;
    Mock::given(method("POST"))
        .respond_with(created_as("T1-copy"))
        .mount(&graph.server)
        .await;
    env.synced();

    let deleted = env.json(&["tasks", "delete", "T1", "--yes"]);
    env.op_in_state(&op_id(&deleted), "inflight");
    // Queued behind the delete Graph hasn't answered yet.
    let undone = env.json(&["undo"]);
    let recreate = op_id(&undone);

    env.op_in_state(&op_id(&deleted), "failed");
    let cascaded = env.op_in_state(&recreate, "failed");
    assert!(
        cascaded["note"]
            .as_str()
            .is_some_and(|note| note.contains(&op_id(&deleted))),
        "{cascaded}"
    );
    env.synced();
    env.settled();
    assert!(writes_to(&graph, "POST").await.is_empty(), "no second copy");
    let listed = tasks(&env, "Tasks");
    assert_eq!(listed.len(), 1, "the task Graph kept is still there");
    assert_eq!(listed[0]["graph_id"], "T1");
}
