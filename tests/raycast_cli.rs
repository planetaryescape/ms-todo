//! Exercise the Raycast argument contract through the real binary and daemon.

mod support;

use chrono::Duration;
use support::Env;
use support::fake_graph::{FakeGraph, list};

async fn graph(env: &mut Env) -> FakeGraph {
    let graph = FakeGraph::start(
        env,
        vec![
            list("L-tasks", "Tasks", "defaultList"),
            list("L-home", "Home", "none"),
        ],
    )
    .await;
    graph.edit(|data| {
        data.tasks.insert("L-tasks".into(), Vec::new());
        data.tasks.insert("L-home".into(), Vec::new());
    });
    graph.accept_task_patches().await;
    graph.accept_moves().await;
    graph
}

#[tokio::test]
async fn raycast_capture_and_rename_keep_hyphen_prefixed_titles_literal() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    env.synced();

    let added = env.json(&[
        "tasks",
        "add",
        "--list",
        "Home",
        "--strict",
        "--",
        "--help with the garden",
    ]);
    let id = added["items"][0]["id"].as_str().expect("local ID");
    assert_eq!(added["items"][0]["title"], "--help with the garden");
    env.settled();
    assert_eq!(
        graph.tasks_in("L-home")[0]["title"],
        "--help with the garden"
    );

    let renamed = env.json(&["tasks", "edit", id, "--title=--help with the flowers"]);
    assert_eq!(renamed["items"][0]["title"], "--help with the flowers");
    env.settled();
    assert_eq!(
        graph.tasks_in("L-home")[0]["title"],
        "--help with the flowers"
    );
}

#[tokio::test]
async fn raycast_search_includes_deferred_tasks_across_lists_while_browse_hides_them() {
    let mut env = Env::new();
    let _graph = graph(&mut env).await;
    env.synced();
    let tomorrow = (env.today() + Duration::days(1))
        .format("%Y-%m-%d")
        .to_string();

    env.json(&["tasks", "add", "--strict", "--", "Plan the trip ^tomorrow"]);
    env.json(&[
        "tasks",
        "add",
        "--list",
        "Home",
        "--strict",
        "--",
        "Learn the cello +someday",
    ]);
    env.json(&["tasks", "add", "--strict", "--", "Renew passport"]);
    env.settled();

    let search = env.json(&["tasks", "list", "--status", "all", "--deferred", "include"]);
    let items = search["items"].as_array().expect("search items");
    assert_eq!(items.len(), 3);
    let deferred = items
        .iter()
        .find(|task| task["title"] == "Plan the trip")
        .expect("deferred task");
    assert_eq!(deferred["defer_until"], tomorrow);
    let someday = items
        .iter()
        .find(|task| task["title"] == "Learn the cello")
        .expect("Someday task");
    assert_eq!(someday["someday"], true);
    assert_eq!(someday["list"], "Home");

    let browse = env.json(&["tasks", "list", "--status", "open"]);
    assert_eq!(browse["items"].as_array().map(Vec::len), Some(1));
    assert_eq!(browse["items"][0]["title"], "Renew passport");
    assert_eq!(browse["deferred_hidden"], 2);
}
