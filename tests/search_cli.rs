//! `search` and `tasks list --search` through the real binary and daemon,
//! against a fake Graph: every list by default, the filters, exit codes,
//! and each output format.

mod support;

use serde_json::{Value, json};
use support::Env;
use support::fake_graph::{FakeGraph, list, task};

fn noted(id: &str, title: &str, body: &str) -> Value {
    let mut task = task(id, title, &format!("W/\"{id}\""));
    task["body"] = json!({ "content": body, "contentType": "text" });
    task
}

/// "Tasks" and "Home", with insurance mentioned in titles and notes of
/// both, one of them completed, synced.
async fn synced_env() -> (Env, FakeGraph) {
    let mut env = Env::new();
    let graph = FakeGraph::start(
        &mut env,
        vec![
            list("L-tasks", "Tasks", "defaultList"),
            list("L-home", "Home", "none"),
        ],
    )
    .await;
    let mut done = task("T3", "Old insurance claim", "W/\"T3\"");
    done["status"] = json!("completed");
    let mut due = task("H1", "Renew car insurance", "W/\"H1\"");
    // Midnight in London during BST (S11).
    due["dueDateTime"] = json!({ "dateTime": "2026-09-25T23:00:00.0000000", "timeZone": "UTC" });
    graph.edit(|data| {
        data.tasks.insert(
            "L-tasks".into(),
            vec![
                noted(
                    "T1",
                    "Call the broker",
                    "Ask about the insurance, \"fully\" comp",
                ),
                task("T2", "Buy milk", "W/\"T2\""),
                done,
            ],
        );
        data.tasks.insert("L-home".into(), vec![due]);
    });
    env.synced();
    (env, graph)
}

fn graph_ids(result: &Value) -> Vec<&str> {
    result["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| item["graph_id"].as_str().expect("graph_id"))
        .collect()
}

#[tokio::test]
async fn search_finds_open_tasks_in_every_list_best_match_first() {
    let (env, _graph) = synced_env().await;
    let result = env.json(&["search", "insurance"]);
    assert_eq!(result["schema_version"], 2);
    assert_eq!(result["sync"]["state"], "ready");
    // The title match first; the completed task isn't open.
    assert_eq!(graph_ids(&result), ["H1", "T1"]);
    let first = &result["items"][0];
    assert_eq!(first["list"], "Home");
    assert_eq!(first["snippet"], "Renew car **insurance**");
    assert_eq!(first["title"], "Renew car insurance");
    assert!(first["id"].is_string() && first["list_id"].is_string());
    assert_eq!(result["items"][1]["list"], "Tasks");

    // Several words are all required, typed with or without quotes.
    assert_eq!(
        graph_ids(&env.json(&["search", "car", "insurance"])),
        ["H1"]
    );
    assert_eq!(
        graph_ids(&env.json(&["search", "insur*", "claim"])),
        [] as [&str; 0]
    );
    assert_eq!(
        graph_ids(&env.json(&["search", "insur*", "claim", "--status", "all"])),
        ["T3"]
    );
    assert_eq!(
        graph_ids(&env.json(&["search", "\"car insurance\""])),
        ["H1"]
    );
    assert_eq!(graph_ids(&env.json(&["search", "milk OR broker"])).len(), 2);
}

#[tokio::test]
async fn filters_narrow_the_search() {
    let (env, _graph) = synced_env().await;
    assert_eq!(
        graph_ids(&env.json(&["search", "insurance", "--list", "Tasks"])),
        ["T1"]
    );
    assert_eq!(
        graph_ids(&env.json(&["search", "insurance", "--status", "completed"])),
        ["T3"]
    );
    assert_eq!(
        env.json(&["search", "insurance", "--status", "all", "--limit", "1"])["items"]
            .as_array()
            .expect("items")
            .len(),
        1
    );
    let error = env.failure(&["search", "insurance", "--list", "Nope"], 3);
    assert_eq!(error["error"]["kind"], "not_found");
    env.cmd()
        .args(["search", "insurance", "--limit", "0"])
        .assert()
        .code(2);
}

#[tokio::test]
async fn tasks_list_search_ranks_one_lists_tasks_whatever_their_status() {
    let (env, _graph) = synced_env().await;
    let result = env.json(&["tasks", "list", "--search", "insurance"]);
    assert_eq!(result["sync"]["state"], "ready");
    // Title matches (T3, completed) above the body match; not Home's.
    assert_eq!(graph_ids(&result), ["T3", "T1"]);
    assert!(
        result["items"][0].get("snippet").is_none(),
        "the tasks shape"
    );
    let home = env.json(&["tasks", "list", "--list", "Home", "--search", "insurance"]);
    assert_eq!(graph_ids(&home), ["H1"]);
}

#[tokio::test]
async fn a_malformed_query_exits_2_with_a_clear_message() {
    let (env, _graph) = synced_env().await;
    for query in ["OR insurance", "(insurance", "\"insurance", "--"] {
        let error = env.failure(&["search", "--", query], 2);
        assert_eq!(error["error"]["kind"], "invalid_input", "{query}");
        let message = error["error"]["message"].as_str().expect("message");
        assert!(!message.contains("database"), "{query}: {message}");
    }
    let error = env.failure(&["tasks", "list", "--search", "NOT"], 2);
    assert_eq!(error["error"]["kind"], "invalid_input");
    // Punctuation in a word is searched for, not rejected.
    env.json(&["search", "e-mail", "don't", "v1.2"]);
}

#[tokio::test]
async fn every_output_format() {
    let (env, _graph) = synced_env().await;
    let t1 = env.local_id(&["tasks", "list"], "T1");
    let h1 = env.local_id(&["tasks", "list", "--list", "Home"], "H1");
    let run = |format: &str| {
        let output = env
            .cmd()
            .args(["--format", format, "search", "insurance"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        String::from_utf8(output)
            .expect("utf8")
            .replace(&t1, "<local id of T1>")
            .replace(&h1, "<local id of H1>")
    };
    // stdout isn't a terminal here, so the table keeps the ** marks.
    insta::assert_snapshot!("search_table", run("table"));
    insta::assert_snapshot!("search_csv", run("csv"));
    insta::assert_snapshot!("search_ids", run("ids"));
    let jsonl: Vec<Value> = run("jsonl")
        .lines()
        .map(|line| serde_json::from_str(line).expect("json line"))
        .collect();
    assert_eq!(jsonl.len(), 2);
    assert!(jsonl.iter().all(|line| line["schema_version"] == 2));
    assert_eq!(jsonl[0]["snippet"], "Renew car **insurance**");
    let json: Value = serde_json::from_str(&run("json")).expect("json");
    assert_eq!(json["items"].as_array().expect("items").len(), 2);
}
