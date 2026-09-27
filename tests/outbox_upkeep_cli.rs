//! Keeping the outbox (D-065) through the real binary and daemon: finished
//! writes are pruned when the daemon starts once they're older than
//! `[outbox] retention_days`, `doctor` shows the outbox's size and
//! settings, and a bad setting keeps the defaults with a problem.

mod support;

use serde_json::{Value, json};
use support::Env;
use support::fake_graph::{FakeGraph, list, task};

async fn graph_with(env: &mut Env, tasks: Vec<Value>) -> FakeGraph {
    let graph = FakeGraph::start(env, vec![list("L-tasks", "Tasks", "defaultList")]).await;
    graph.edit(|data| {
        data.tasks.insert("L-tasks".into(), tasks);
    });
    graph.accept_task_patches().await;
    graph
}

fn write_config(env: &Env, contents: &str) {
    let dir = env.home.path().join("config").join("ms-todo");
    std::fs::create_dir_all(&dir).expect("config dir");
    std::fs::write(dir.join("config.toml"), contents).expect("config");
}

/// Make every finished operation look 40 days old.
async fn age_the_outbox(env: &Env) {
    let database = env.json(&["doctor"])["database"]["path"]
        .as_str()
        .expect("database path")
        .to_owned();
    env.json(&["daemon", "stop"]);
    let mut connection =
        <sqlx::SqliteConnection as sqlx::Connection>::connect(&format!("sqlite:{database}"))
            .await
            .expect("open the database");
    let old = chrono::Utc::now().timestamp() - 40 * 24 * 60 * 60;
    sqlx::query("UPDATE outbox SET finished_at = ?, created_at = ? WHERE state = 'done'")
        .bind(old)
        .bind(old)
        .execute(&mut connection)
        .await
        .expect("age");
}

#[tokio::test]
async fn finished_writes_older_than_the_window_are_pruned_when_the_daemon_starts() {
    let mut env = Env::new();
    let _graph = graph_with(&mut env, vec![task("T1", "Buy milk", "W/\"e1\"")]).await;
    env.synced();
    let edited = env.json(&["tasks", "edit", "T1", "--title", "Oat milk"]);
    let op_id = edited["op_id"].as_str().expect("op_id").to_owned();
    env.settled();
    let doctor = env.json(&["doctor"]);
    let upkeep = &doctor["outbox_upkeep"];
    assert_eq!(upkeep["rows"], 1, "{doctor}");
    assert_eq!(upkeep["retention_days"], 30);
    assert_eq!(upkeep["unknown_lookup_hours"], 24);
    assert_eq!(upkeep["last_pruned"], 0, "pruned at start, nothing old yet");

    age_the_outbox(&env).await;

    let doctor = env.json(&["doctor"]);
    assert_eq!(doctor["outbox_upkeep"]["last_pruned"], 1, "{doctor}");
    assert_eq!(doctor["outbox_upkeep"]["rows"], 0);
    assert!(env.outbox().iter().all(|op| op["op_id"] != op_id.as_str()));
    // Past the window, undo has nothing to reach.
    let error = env.failure(&["undo", &op_id], 3);
    assert_eq!(error["error"]["kind"], "not_found", "{error}");
    let log = env.json(&["daemon", "logs"]).to_string();
    assert!(log.contains("pruned 1 operation(s)"), "{log}");
}

#[tokio::test]
async fn the_outbox_settings_are_read_from_config_and_a_bad_one_keeps_the_defaults() {
    let mut env = Env::new();
    let _graph = graph_with(&mut env, Vec::new()).await;
    write_config(
        &env,
        "[outbox]\nretention_days = 7\nunknown_lookup_hours = 2\n",
    );
    let doctor = env.json(&["doctor"]);
    let upkeep = &doctor["outbox_upkeep"];
    assert_eq!(
        (&upkeep["retention_days"], &upkeep["unknown_lookup_hours"]),
        (&json!(7), &json!(2)),
        "{doctor}"
    );

    env.json(&["daemon", "stop"]);
    write_config(&env, "[outbox]\nretention_days = 0\n");
    let doctor = env.json(&["doctor"]);
    assert_eq!(doctor["outbox_upkeep"]["retention_days"], 30, "{doctor}");
    let problems = doctor["problems"].to_string();
    assert!(
        problems.contains("outbox:") && problems.contains("1 to 3650"),
        "{problems}"
    );
}
