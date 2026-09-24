//! `--idempotency-key` through the real binary and daemon: a repeat gets
//! the first result without queueing it again, the same key with a
//! different request exits 2, and a request that failed, which queued
//! nothing, frees the key (docs/blueprint/04-sync-cache.md#instant-local-writes).

mod support;

use serde_json::{Value, json};
use support::Env;
use support::fake_graph::{FakeGraph, list, task};
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, Request, ResponseTemplate};

const LIST: &str = "/v1.0/me/todo/lists/L-tasks/tasks";

async fn graph(env: &mut Env) -> FakeGraph {
    let graph = FakeGraph::start(env, vec![list("L-tasks", "Tasks", "defaultList")]).await;
    graph.edit(|data| {
        data.tasks
            .insert("L-tasks".into(), vec![task("T1", "Buy milk", "W/\"e1\"")]);
    });
    Mock::given(method("POST"))
        .and(path(LIST))
        .and(body_partial_json(json!({ "title": "Buy bread" })))
        .respond_with(|request: &Request| {
            let mut created: Value = serde_json::from_slice(&request.body).expect("json body");
            created["id"] = json!("T-new");
            created["@odata.etag"] = json!("W/\"n1\"");
            ResponseTemplate::new(201).set_body_json(created)
        })
        .mount(&graph.server)
        .await;
    graph
}

#[tokio::test]
async fn a_repeat_with_the_same_key_returns_the_first_result_without_resending() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    let add = [
        "tasks",
        "add",
        "Buy bread",
        "--idempotency-key",
        "agent-run-1",
    ];

    let first = env.json(&add);
    env.settled();
    let second = env.json(&add);

    assert_eq!(first, second, "the same op_id and the same task");
    env.settled();
    assert_eq!(graph.writes().await.len(), 1);
    assert_eq!(env.outbox().len(), 1, "queued once");

    let mut done = task("T1", "Buy milk", "W/\"e2\"");
    done["status"] = json!("completed");
    Mock::given(method("PATCH"))
        .and(path(format!("{LIST}/T1")))
        .respond_with(ResponseTemplate::new(200).set_body_json(done))
        .mount(&graph.server)
        .await;
    let complete = [
        "tasks",
        "complete",
        "T1",
        "--idempotency-key",
        "agent-run-2",
    ];
    assert_eq!(env.json(&complete), env.json(&complete));
    env.settled();
    assert_eq!(graph.writes().await.len(), 2);
}

#[tokio::test]
async fn the_same_key_for_a_different_request_exits_2() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    env.json(&["tasks", "add", "Buy bread", "--idempotency-key", "k"]);

    let output = env
        .cmd()
        .args([
            "--format",
            "json",
            "tasks",
            "add",
            "Buy bread rolls",
            "--idempotency-key",
            "k",
        ])
        .assert()
        .code(2)
        .get_output()
        .clone();

    let error: Value = serde_json::from_slice(&output.stderr).expect("json error");
    assert_eq!(error["error"]["kind"], "invalid_input");
    assert!(
        error["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("different request")),
        "{error}"
    );
    env.settled();
    assert_eq!(graph.writes().await.len(), 1);
}

#[tokio::test]
async fn a_request_that_failed_frees_the_key() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    let add = [
        "tasks",
        "add",
        "Buy bread",
        "--list",
        "Groceries",
        "--idempotency-key",
        "k",
    ];

    // No such list yet: nothing is queued.
    env.failure(&add, 3);
    graph.edit(|data| {
        data.lists.push(list("L-groc", "Groceries", "none"));
        data.tasks.insert("L-groc".into(), Vec::new());
    });
    env.synced();
    Mock::given(method("POST"))
        .and(path("/v1.0/me/todo/lists/L-groc/tasks"))
        .respond_with(ResponseTemplate::new(201).set_body_json(task("T-b", "Buy bread", "W/\"b\"")))
        .mount(&graph.server)
        .await;
    let retried = env.json(&add);

    assert_eq!(retried["action"], "add");
    env.settled();
    assert_eq!(graph.writes().await.len(), 1);
}

#[tokio::test]
async fn a_write_the_cache_cant_queue_reaches_nothing_and_frees_its_key() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    env.synced();
    // Make the daemon's next task insert fail, as a full disk would.
    let database = env.json(&["doctor"])["database"]["path"]
        .as_str()
        .expect("database path")
        .to_owned();
    let mut connection =
        <sqlx::SqliteConnection as sqlx::Connection>::connect(&format!("sqlite:{database}"))
            .await
            .expect("open the daemon's database");
    sqlx::query(
        "CREATE TRIGGER refuse_tasks BEFORE INSERT ON tasks \
         BEGIN SELECT RAISE(ABORT, 'test: the cache refuses writes'); END",
    )
    .execute(&mut connection)
    .await
    .expect("trigger");
    let add = ["tasks", "add", "Buy bread", "--idempotency-key", "k"];

    let refused = env.failure(&add, 1);
    assert_eq!(refused["error"]["kind"], "internal", "{refused}");
    assert!(env.outbox().is_empty(), "nothing queued");

    sqlx::query("DROP TRIGGER refuse_tasks")
        .execute(&mut connection)
        .await
        .expect("drop trigger");
    env.json(&add);
    env.settled();
    assert_eq!(graph.writes().await.len(), 1, "one POST, from the retry");
}
