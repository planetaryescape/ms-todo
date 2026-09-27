//! Semantic search (rung 9c, D-062) through the real binary and daemon,
//! against a fake Graph and a made-up model (`support::tiny_model`):
//! off until config turns it on, tasks found by meaning with no word in
//! common, re-embedding a task whose text changed, `doctor`, and the
//! output formats. The tasks here are made up.

mod support;

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use support::Env;
use support::fake_graph::{FakeGraph, list, task};

fn write_config(env: &Env, contents: &str) {
    let dir = env.home.path().join("config").join("ms-todo");
    std::fs::create_dir_all(&dir).expect("config dir");
    std::fs::write(dir.join("config.toml"), contents).expect("config");
}

/// Three tasks in "Tasks", one of them completed, with semantic search on
/// and the tiny model, synced and indexed.
async fn indexed_env() -> (Env, FakeGraph) {
    let mut env = Env::new();
    let model = env.home.path().join("tiny-model");
    support::tiny_model::write(&model);
    env.semantic_model_dir = Some(model);
    write_config(&env, "[search]\nsemantic = true\n");
    let mut done = task("T3", "Buy milk", "W/\"T3\"");
    done["status"] = json!("completed");
    let graph = FakeGraph::start(&mut env, vec![list("L-tasks", "Tasks", "defaultList")]).await;
    graph.edit(|data| {
        data.tasks.insert(
            "L-tasks".into(),
            vec![
                task("T1", "Book teeth cleaning", "W/\"T1\""),
                task("T2", "Renew car insurance", "W/\"T2\""),
                done,
            ],
        );
    });
    env.synced();
    indexed(&env, 3);
    (env, graph)
}

/// Wait up to 10 seconds until `count` tasks are embedded and none wait.
fn indexed(env: &Env, count: u64) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let semantic = env.json(&["doctor"])["semantic"].clone();
        if semantic["state"] == "ready" && semantic["indexed"] == count && semantic["pending"] == 0
        {
            return semantic;
        }
        assert!(Instant::now() < deadline, "never indexed: {semantic}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn titles(result: &Value) -> Vec<&str> {
    result["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| item["title"].as_str().expect("title"))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn off_by_default_it_says_how_to_turn_it_on_and_downloads_nothing() {
    let mut env = Env::new();
    let _graph = FakeGraph::start(&mut env, vec![list("L-tasks", "Tasks", "defaultList")]).await;
    let error = env.failure(&["search", "dentist", "--semantic"], 2);
    assert_eq!(error["error"]["kind"], "invalid_input");
    let message = error["error"]["message"].as_str().expect("message");
    assert!(message.contains("semantic = true"), "{message}");
    assert!(message.contains("[search]"), "{message}");
    assert!(message.contains("config.toml"), "{message}");

    let semantic = env.json(&["doctor"])["semantic"].clone();
    assert_eq!(semantic["enabled"], false);
    assert_eq!(semantic["state"], "off");
    assert_eq!(semantic["model"], "potion-base-8M@bf8b056");
    let model_dir = semantic["model_dir"].as_str().expect("model_dir");
    assert!(
        !std::path::Path::new(model_dir).exists(),
        "nothing downloaded"
    );
    // Keyword search is untouched.
    env.synced();
    env.json(&["search", "dentist"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn tasks_are_found_by_meaning_with_no_word_in_common() {
    let (env, _graph) = indexed_env().await;
    let found = env.json(&["search", "dentist", "--semantic"]);
    assert_eq!(titles(&found), ["Book teeth cleaning"]);
    let hit = &found["items"][0];
    assert_eq!(hit["list"], "Tasks");
    assert!(hit["score"].as_f64().expect("score") > 0.5, "{hit}");
    assert!(hit.get("snippet").is_none(), "{hit}");
    assert_eq!(found["semantic"]["model"], "potion-base-8M@bf8b056");
    assert_eq!(found["semantic"]["pending"], 0);

    assert_eq!(
        titles(&env.json(&["search", "vehicle", "--semantic"])),
        ["Renew car insurance"]
    );
    // Open tasks only unless asked, as keyword search.
    assert!(titles(&env.json(&["search", "groceries", "--semantic"])).is_empty());
    assert_eq!(
        titles(&env.json(&["search", "groceries", "--semantic", "--status", "all"])),
        ["Buy milk"]
    );
    // A word the model doesn't know is close to nothing.
    assert!(titles(&env.json(&["search", "zebra", "--semantic"])).is_empty());
    // The same words by keyword find nothing: no word matches.
    assert!(titles(&env.json(&["search", "dentist"])).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn results_are_ranked_and_limited_in_every_format() {
    let (env, _graph) = indexed_env().await;
    // "renew" is on the axis both open tasks share: the teeth cleaning is
    // more of it (0.79) than the insurance (0.71).
    let found = env.json(&["search", "renew", "--semantic"]);
    assert_eq!(
        titles(&found),
        ["Book teeth cleaning", "Renew car insurance"]
    );
    let scores: Vec<f64> = found["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| item["score"].as_f64().expect("score"))
        .collect();
    assert!(scores[0] > scores[1], "{scores:?}");
    let limited = env.json(&["search", "renew", "--semantic", "--limit", "1"]);
    assert_eq!(titles(&limited), ["Book teeth cleaning"]);

    let csv = env
        .cmd()
        .args(["--format", "csv", "search", "dentist", "--semantic"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let csv = String::from_utf8(csv).expect("utf-8");
    let mut lines = csv.lines();
    assert_eq!(lines.next(), Some("id,title,list,status,due,score"));
    assert!(
        lines
            .next()
            .is_some_and(|row| row.contains(",Book teeth cleaning,Tasks,notStarted,,0.")),
        "{csv}"
    );

    let table = env
        .cmd()
        .args(["--format", "table", "search", "dentist", "--semantic"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let table = String::from_utf8(table).expect("utf-8");
    assert!(table.contains("SCORE"), "{table}");
    assert!(table.contains("Book teeth cleaning"), "{table}");

    let jsonl = env
        .cmd()
        .args(["--format", "jsonl", "search", "dentist", "--semantic"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let record: Value = serde_json::from_slice(&jsonl).expect("one record");
    assert_eq!(record["title"], "Book teeth cleaning");
    assert!(record["score"].is_number());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_task_whose_text_changes_is_embedded_again_after_the_sync() {
    let (env, graph) = indexed_env().await;
    graph.edit(|data| {
        let tasks = data.tasks.get_mut("L-tasks").expect("tasks");
        let renamed = tasks
            .iter_mut()
            .find(|task| task["id"] == "T2")
            .expect("T2");
        renamed["title"] = json!("Buy groceries");
        renamed["@odata.etag"] = json!("W/\"T2b\"");
    });
    env.synced();
    indexed(&env, 3);
    assert_eq!(
        titles(&env.json(&["search", "milk", "--semantic"])),
        ["Buy groceries"]
    );
    assert!(titles(&env.json(&["search", "vehicle", "--semantic"])).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn deferred_and_someday_tasks_are_found_and_marked() {
    let (env, _graph) = indexed_env().await;
    let teeth = env.local_id(&["tasks", "list"], "T1");
    let insurance = env.local_id(&["tasks", "list"], "T2");
    env.json(&["tasks", "edit", &teeth, "--defer", "2099-01-01"]);
    env.json(&["tasks", "edit", &insurance, "--someday"]);
    // Out of `tasks list`, as rung 9a has it, but a search finds them.
    assert_eq!(titles(&env.json(&["tasks", "list"])), ["Buy milk"]);
    let found = env.json(&["search", "dentist", "--semantic"]);
    assert_eq!(titles(&found), ["Book teeth cleaning"]);
    assert_eq!(found["items"][0]["defer_until"], "2099-01-01");
    let table = env
        .cmd()
        .args(["--format", "table", "search", "vehicle", "--semantic"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let table = String::from_utf8(table).expect("utf-8");
    assert!(table.contains("Renew car insurance (someday)"), "{table}");
}
