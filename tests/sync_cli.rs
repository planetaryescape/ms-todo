//! Full-enumeration sync through the real binary and daemon, against a fake
//! Graph: the envelope's `initial` state, paging, tombstones, the
//! extension fetch and its checkpoint rule, writes made during a pass, and
//! `doctor` (docs/blueprint/04-sync-cache.md).

mod support;

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use support::Env;
use support::fake_graph::{FakeGraph, graph_with_tasks, list, task};
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

const EXTENSION: &str = "com.planetaryescape.mstodo";

fn graph_ids(collection: &Value) -> Vec<String> {
    collection["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter_map(|item| item["graph_id"].as_str().map(str::to_owned))
        .collect()
}

#[tokio::test]
async fn the_envelope_is_initial_until_the_first_sync_finishes_then_ready() {
    let mut env = Env::new();
    let graph = graph_with_tasks(&mut env, vec![task("T1", "Buy milk", "W/\"e1\"")]).await;
    graph.edit(|data| data.tasks_delay = Some(Duration::from_millis(1500)));

    let first = env.json(&["tasks", "list"]);
    assert_eq!(
        first["sync"],
        json!({ "state": "initial", "generation": 0 })
    );
    assert_eq!(first["items"], json!([]), "empty, but not claimed to be");

    let table = env
        .cmd()
        .args(["--format", "table", "tasks", "list"])
        .assert()
        .success()
        .get_output()
        .clone();
    let note = String::from_utf8(table.stderr).expect("utf8");
    assert!(note.contains("still syncing"), "{note}");

    graph.edit(|data| data.tasks_delay = None);
    env.synced();
    let ready = env.json(&["tasks", "list"]);
    assert_eq!(ready["sync"]["state"], "ready");
    assert!(
        ready["sync"]["generation"]
            .as_u64()
            .is_some_and(|generation| generation >= 1)
    );
    assert_eq!(graph_ids(&ready), ["T1"]);
}

#[tokio::test]
async fn a_sync_pages_every_task_and_brings_in_changes_additions_and_deletions() {
    let mut env = Env::new();
    let tasks = (1..=5)
        .map(|n| task(&format!("T{n}"), &format!("task {n}"), "W/\"e1\""))
        .collect();
    let graph = graph_with_tasks(&mut env, tasks).await;
    graph.edit(|data| data.page_size = 2);

    let first = env.synced();
    assert_eq!(first["waited"], true);
    assert_eq!(first["scopes"], 2, "the lists, and one list's tasks");
    let listed = env.json(&["tasks", "list"]);
    assert_eq!(graph_ids(&listed), ["T1", "T2", "T3", "T4", "T5"]);
    let t3 = env.local_id(&["tasks", "list"], "T3");

    // Changed on the phone: T2 deleted, T3 renamed, T6 added.
    graph.edit(|data| {
        let tasks = data.tasks.get_mut("L-tasks").expect("tasks");
        tasks.retain(|task| task["id"] != "T2");
        tasks[1]["title"] = json!("task 3, renamed");
        tasks[1]["@odata.etag"] = json!("W/\"e2\"");
        tasks.push(task("T6", "task 6", "W/\"e1\""));
    });
    let second = env.synced();
    assert_eq!(second["changed"], 3);
    assert!(second["generation"].as_u64() > first["generation"].as_u64());

    let listed = env.json(&["tasks", "list"]);
    assert_eq!(graph_ids(&listed), ["T1", "T3", "T4", "T5", "T6"]);
    assert_eq!(
        env.local_id(&["tasks", "list"], "T3"),
        t3,
        "the local ID is stable"
    );
    assert_eq!(listed["items"][1]["title"], "task 3, renamed");

    // The first pass read the list whole in three pages of two, then an
    // empty page with the deltaLink (P1). Later passes replay that link.
    // `Prefer` goes on every page, since Graph doesn't carry it (P1).
    let starts = graph.delta_starts("L-tasks").await;
    assert_eq!(starts[0], None, "the first pass starts fresh");
    assert!(
        starts[1..].iter().all(Option::is_some),
        "then delta: {starts:?}"
    );
    let pages = graph.delta_pages().await;
    let queries: Vec<String> = pages
        .iter()
        .filter(|page| page.url.path() == "/v1.0/me/todo/lists/L-tasks/tasks/delta")
        .map(|page| page.url.query().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(queries[0], "", "a fresh round sends no query options (S3)");
    assert!(
        queries[1..4]
            .iter()
            .all(|query| query.starts_with("$skiptoken=")),
        "{queries:?}"
    );
    assert!(queries[4].starts_with("$deltatoken="), "{queries:?}");
    assert!(pages.iter().all(|page| {
        page.headers
            .get("Prefer")
            .is_some_and(|prefer| prefer == "odata.maxpagesize=200")
    }));
}

#[tokio::test]
async fn a_list_deleted_elsewhere_takes_its_tasks_with_it() {
    let mut env = Env::new();
    let graph = graph_with_tasks(&mut env, Vec::new()).await;
    graph.edit(|data| {
        data.lists.push(list("L-groc", "Groceries", "none"));
        data.tasks
            .insert("L-groc".into(), vec![task("T9", "Eggs", "W/\"g1\"")]);
    });
    env.synced();
    let groceries = env.local_id(&["lists", "list"], "L-groc");
    assert_eq!(
        env.json(&["doctor"])["scopes"].as_array().map(Vec::len),
        Some(3)
    );

    graph.edit(|data| data.lists.retain(|list| list["id"] != "L-groc"));
    env.synced();

    assert_eq!(graph_ids(&env.json(&["lists", "list"])), ["L-tasks"]);
    let error = env.failure(&["tasks", "list", "--list", &groceries], 3);
    assert_eq!(error["error"]["kind"], "not_found");
    env.failure(&["tasks", "complete", "T9"], 3);
    assert_eq!(
        env.json(&["doctor"])["scopes"].as_array().map(Vec::len),
        Some(2),
        "its tasks scope is gone too"
    );
}

#[tokio::test]
async fn our_extension_is_fetched_once_per_change_and_shown() {
    let mut env = Env::new();
    let graph = graph_with_tasks(
        &mut env,
        vec![
            task("T1", "Buy milk", "W/\"e1\""),
            task("T2", "Eggs", "W/\"e1\""),
        ],
    )
    .await;
    graph.edit(|data| {
        data.extensions.insert(
            "T1".into(),
            json!({
                "extensionName": EXTENSION,
                "id": format!("microsoft.graph.openTypeExtension.{EXTENSION}"),
                "opId": "op-1"
            }),
        );
    });

    env.synced();
    assert_eq!(graph.batched_gets().await.len(), 2);
    let listed = env.json(&["tasks", "list"]);
    assert_eq!(listed["items"][0]["extensions"][0]["opId"], "op-1");
    assert!(listed["items"][1].get("extensions").is_none());
    assert!(
        graph.batched_gets().await[0]
            .contains("$expand=extensions($filter=id%20eq%20%27com.planetaryescape.mstodo%27)"),
        "the filtered $expand (S2)"
    );

    // Nothing changed: nothing to fetch.
    env.synced();
    assert_eq!(graph.batched_gets().await.len(), 2);

    // Only the task whose etag moved is fetched again.
    graph.edit(|data| {
        data.tasks.get_mut("L-tasks").expect("tasks")[1]["@odata.etag"] = json!("W/\"e2\"");
    });
    env.synced();
    let fetched = graph.batched_gets().await;
    assert_eq!(fetched.len(), 3);
    assert!(
        fetched[2].starts_with("/me/todo/lists/L-tasks/tasks/T2?"),
        "{}",
        fetched[2]
    );
}

#[tokio::test]
async fn a_task_deleted_before_its_extension_fetch_is_tombstoned_and_the_sync_succeeds() {
    let mut env = Env::new();
    let graph = graph_with_tasks(
        &mut env,
        vec![
            task("T1", "Buy milk", "W/\"e1\""),
            task("T2", "Eggs", "W/\"e1\""),
        ],
    )
    .await;
    graph.edit(|data| {
        data.gone_on_fetch.insert("T2".into());
    });

    env.synced();

    let listed = env.json(&["tasks", "list"]);
    assert_eq!(listed["sync"]["state"], "ready");
    assert_eq!(graph_ids(&listed), ["T1"]);
}

#[tokio::test]
async fn a_failed_extension_fetch_applies_nothing_to_the_checkpoint() {
    let mut env = Env::new();
    let graph = graph_with_tasks(&mut env, vec![task("T1", "Buy milk", "W/\"e1\"")]).await;
    graph.edit(|data| {
        data.forbidden_on_fetch.insert("T1".into());
    });

    let error = env.failure(&["sync", "--wait"], 5);
    assert_eq!(error["error"]["kind"], "rejected");
    let doctor = env.json(&["doctor"]);
    let tasks_scope = doctor["scopes"]
        .as_array()
        .expect("scopes")
        .iter()
        .find(|scope| scope["scope"] == "tasks:L-tasks")
        .cloned()
        .expect("tasks scope");
    assert_eq!(tasks_scope["state"], "initial", "no checkpoint");
    assert_eq!(tasks_scope["generation"], 0);
    assert_eq!(tasks_scope["last_error"]["kind"], "rejected");
    assert_eq!(doctor["last_error"]["scope"], "tasks:L-tasks");
    assert!(!doctor["problems"].as_array().expect("problems").is_empty());
    // A read of a scope that never synced says why, rather than `initial`.
    env.failure(&["tasks", "list"], 5);

    graph.edit(|data| data.forbidden_on_fetch.clear());
    env.synced();
    let listed = env.json(&["tasks", "list"]);
    assert_eq!(listed["sync"], json!({ "state": "ready", "generation": 1 }));
    assert_eq!(graph_ids(&listed), ["T1"]);
}

#[tokio::test]
async fn a_task_added_while_a_sync_is_in_flight_is_not_tombstoned_by_it() {
    let mut env = Env::new();
    let graph = graph_with_tasks(&mut env, vec![task("T1", "Buy milk", "W/\"e1\"")]).await;
    Mock::given(method("POST"))
        .and(path("/v1.0/me/todo/lists/L-tasks/tasks"))
        .respond_with(ResponseTemplate::new(201).set_body_json(task(
            "T-new",
            "Buy bread",
            "W/\"n1\"",
        )))
        .mount(&graph.server)
        .await;
    graph.edit(|data| data.tasks_delay = Some(Duration::from_millis(1500)));

    // The daemon's first sync fetches the lists, then waits on the tasks
    // page, which doesn't have T-new. Add it meanwhile.
    env.json(&["daemon", "start"]);
    let lists_ready = Instant::now() + Duration::from_secs(5);
    while env.json(&["lists", "list"])["sync"]["state"] != "ready" {
        assert!(Instant::now() < lists_ready, "the lists never synced");
        std::thread::sleep(Duration::from_millis(50));
    }
    let added = env.json(&["tasks", "add", "Buy bread"]);
    assert_eq!(added["items"][0]["sync_state"], "pending");

    env.settled();
    env.wait_until_idle();
    let listed = env.json(&["tasks", "list"]);
    assert_eq!(listed["sync"]["state"], "ready");
    let mut ids = graph_ids(&listed);
    ids.sort();
    assert_eq!(ids, ["T-new", "T1"]);
}

#[tokio::test]
async fn doctor_reports_sign_in_the_daemon_the_database_and_every_scope() {
    let mut env = Env::new();
    let _graph = graph_with_tasks(&mut env, vec![task("T1", "Buy milk", "W/\"e1\"")]).await;
    env.synced();

    let doctor = env.json(&["doctor"]);

    assert_eq!(doctor["schema_version"], 2);
    assert_eq!(doctor["sign_in"]["signed_in"], true);
    assert_eq!(doctor["daemon"]["ready"], true);
    assert!(
        doctor["database"]["path"]
            .as_str()
            .is_some_and(|path| path.ends_with("ms-todo-dev/ms-todo.db")),
        "{doctor}"
    );
    assert!(
        doctor["database"]["bytes"]
            .as_u64()
            .is_some_and(|bytes| bytes > 0)
    );
    let scopes = doctor["scopes"].as_array().expect("scopes");
    assert_eq!(scopes.len(), 2);
    assert!(scopes.iter().all(|scope| scope["state"] == "ready"));
    assert_eq!(scopes[1]["list_name"], "Tasks");
    assert_eq!(doctor["last_error"], Value::Null);
    assert_eq!(doctor["problems"], json!([]));

    let table = env
        .cmd()
        .args(["--format", "table", "doctor"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let table = String::from_utf8(table).expect("utf8");
    assert!(table.contains("2 of 2 scopes ready"), "{table}");
    assert!(table.contains("all good"), "{table}");
}

/// A title or list name from Graph can hold an escape sequence (here OSC
/// 52, which writes to the clipboard). The table format goes straight to a
/// terminal, so its control characters are replaced; JSON keeps the text.
#[tokio::test]
async fn escape_sequences_in_titles_never_reach_the_terminal_in_a_table() {
    let evil = "Pay\x1b]52;c;aGk=\x07 rent\u{9b}2J";
    let mut env = Env::new();
    let graph = FakeGraph::start(&mut env, vec![list("L-tasks", evil, "defaultList")]).await;
    graph.edit(|data| {
        data.tasks
            .insert("L-tasks".into(), vec![task("T1", evil, "W/\"e1\"")]);
    });
    env.synced();
    assert_eq!(env.json(&["tasks", "list"])["items"][0]["title"], evil);

    for args in [
        &["tasks", "list"][..],
        &["search", "rent"][..],
        &["lists", "list"][..],
    ] {
        let output = env
            .cmd()
            .args(["--format", "table"])
            .args(args)
            .assert()
            .success()
            .get_output()
            .clone();
        let table = String::from_utf8(output.stdout).expect("utf8");
        assert!(
            !table.contains(['\x1b', '\x07', '\u{9b}']),
            "{args:?}: {table:?}"
        );
        assert!(
            table.contains("Pay\u{fffd}]52;c;aGk=\u{fffd} rent\u{fffd}2J"),
            "{args:?}: {table}"
        );
    }
}
