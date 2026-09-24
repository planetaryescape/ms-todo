//! Delta sync through the real binary and daemon, against a fake Graph:
//! delta taking over after the first pass, `@removed`, resets after a
//! rejected link (410, "Badly formed token."), 404 and 5xx that keep the
//! link, deleted and new lists, the checkpoint rule, and a local write
//! racing a delta round (docs/blueprint/04-sync-cache.md#delta-sync).

mod support;

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use support::Env;
use support::fake_graph::{FakeGraph, graph_with_tasks, list, task};
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

fn titles(env: &Env, args: &[&str]) -> Vec<String> {
    let mut titles: Vec<String> = env.json(args)["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter_map(|item| item["title"].as_str().map(str::to_owned))
        .collect();
    titles.sort();
    titles
}

fn scope(env: &Env, name: &str) -> Value {
    env.json(&["doctor"])["scopes"]
        .as_array()
        .expect("scopes")
        .iter()
        .find(|scope| scope["scope"] == name)
        .cloned()
        .unwrap_or(Value::Null)
}

/// The `$deltatoken` of each round of `scope` that replayed a link.
async fn replays(graph: &FakeGraph, scope: &str) -> Vec<String> {
    graph
        .delta_starts(scope)
        .await
        .into_iter()
        .flatten()
        .collect()
}

#[tokio::test]
async fn delta_takes_over_after_the_first_pass_and_brings_in_only_changes() {
    let mut env = Env::new();
    let graph = graph_with_tasks(
        &mut env,
        vec![
            task("T1", "Milk", "W/\"e1\""),
            task("T2", "Eggs", "W/\"e1\""),
        ],
    )
    .await;
    env.synced();
    let doctor = env.json(&["doctor"]);
    for scope in doctor["scopes"].as_array().expect("scopes") {
        assert_eq!(scope["mode"], "delta", "{scope}");
        assert!(scope["last_delta_at"].is_string(), "{scope}");
    }
    let table = env
        .cmd()
        .args(["--format", "table", "doctor"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let table = String::from_utf8(table).expect("utf8");
    assert!(
        table.contains("2 of 2 scopes ready, 2 on delta; last delta "),
        "{table}"
    );
    let hydrated = graph.batched_gets().await.len();

    // On the phone: T1 deleted, T2 renamed, T3 added.
    graph.edit(|data| {
        let tasks = data.tasks.get_mut("L-tasks").expect("tasks");
        tasks.retain(|task| task["id"] != "T1");
        tasks[0]["title"] = json!("Free-range eggs");
        tasks[0]["@odata.etag"] = json!("W/\"e2\"");
        tasks.push(task("T3", "Bread", "W/\"e1\""));
    });
    let report = env.synced();

    assert_eq!(report["changed"], 3);
    assert_eq!(
        titles(&env, &["tasks", "list"]),
        ["Bread", "Free-range eggs"]
    );
    assert_eq!(
        graph.batched_gets().await.len(),
        hydrated + 2,
        "only the tasks delta named are hydrated"
    );
    let starts = graph.delta_starts("L-tasks").await;
    assert_eq!(starts[0], None);
    assert!(starts[1..].iter().all(Option::is_some), "{starts:?}");
    assert_eq!(
        graph
            .requests("GET")
            .await
            .iter()
            .filter(|request| request.url.path() == "/v1.0/me/todo/lists/L-tasks/tasks")
            .count(),
        0,
        "delta replaced the plain enumeration"
    );
}

#[tokio::test]
async fn a_rejected_link_resets_the_scope_and_a_task_deleted_meanwhile_disappears() {
    let mut env = Env::new();
    let graph = graph_with_tasks(
        &mut env,
        vec![
            task("T1", "Milk", "W/\"e1\""),
            task("T2", "Eggs", "W/\"e1\""),
        ],
    )
    .await;
    env.synced();

    // Graph forgets every token (410), T2 is deleted, and T1's extension
    // fetch fails, so the reset can't checkpoint yet.
    graph.edit(|data| {
        data.forget_delta_tokens();
        data.tasks
            .get_mut("L-tasks")
            .expect("tasks")
            .retain(|task| task["id"] != "T2");
        data.tasks.get_mut("L-tasks").expect("tasks")[0]["@odata.etag"] = json!("W/\"e2\"");
        data.forbidden_on_fetch.insert("T1".into());
    });
    env.failure(&["sync", "--wait"], 5);
    assert_eq!(
        scope(&env, "tasks:L-tasks")["mode"],
        "enumeration",
        "the dead link is dropped"
    );
    assert_eq!(
        scope(&env, "lists")["mode"],
        "delta",
        "the lists reset went through"
    );

    graph.edit(|data| data.forbidden_on_fetch.clear());
    env.synced();
    assert_eq!(titles(&env, &["tasks", "list"]), ["Milk"]);
    assert_eq!(scope(&env, "tasks:L-tasks")["mode"], "delta");
    let starts = graph.delta_starts("L-tasks").await;
    assert_eq!(
        starts.iter().filter(|start| start.is_none()).count(),
        3,
        "fresh at first, after the 410, and again after the failed reset: {starts:?}"
    );
}

#[tokio::test]
async fn a_badly_formed_token_resets_the_scope() {
    let mut env = Env::new();
    let graph = graph_with_tasks(
        &mut env,
        vec![
            task("T1", "Milk", "W/\"e1\""),
            task("T2", "Eggs", "W/\"e1\""),
        ],
    )
    .await;
    env.synced();
    graph.edit(|data| {
        data.fail_delta("L-tasks", 400);
        data.tasks
            .get_mut("L-tasks")
            .expect("tasks")
            .retain(|task| task["id"] != "T1");
    });

    env.synced();

    assert_eq!(titles(&env, &["tasks", "list"]), ["Eggs"]);
    let starts = graph.delta_starts("L-tasks").await;
    assert_eq!(starts.last(), Some(&None), "read whole: {starts:?}");
    assert_eq!(scope(&env, "tasks:L-tasks")["mode"], "delta");
}

#[tokio::test]
async fn a_404_on_delta_fails_the_scope_but_keeps_its_link() {
    let mut env = Env::new();
    let graph = graph_with_tasks(&mut env, vec![task("T1", "Milk", "W/\"e1\"")]).await;
    env.synced();
    graph.edit(|data| data.fail_delta("L-tasks", 404));

    let error = env.failure(&["sync", "--wait"], 3);
    assert_eq!(error["error"]["kind"], "not_found");
    let failed = scope(&env, "tasks:L-tasks");
    assert_eq!(failed["mode"], "delta");
    assert_eq!(failed["last_error"]["kind"], "not_found");
    assert_eq!(
        titles(&env, &["tasks", "list"]),
        ["Milk"],
        "the list is still there"
    );

    env.synced();
    let replayed = replays(&graph, "L-tasks").await;
    let [.., failed, retried] = replayed.as_slice() else {
        unreachable!("fewer than two replays: {replayed:?}");
    };
    assert_eq!(failed, retried, "the same link, retried");
    assert_eq!(scope(&env, "tasks:L-tasks")["last_error"], Value::Null);
}

#[tokio::test]
async fn a_5xx_on_delta_is_retried_with_the_same_link() {
    let mut env = Env::new();
    let graph = graph_with_tasks(&mut env, vec![task("T1", "Milk", "W/\"e1\"")]).await;
    env.synced();
    graph.edit(|data| {
        data.fail_delta("L-tasks", 503);
        data.tasks
            .get_mut("L-tasks")
            .expect("tasks")
            .push(task("T2", "Eggs", "W/\"e1\""));
    });

    env.synced();

    assert_eq!(titles(&env, &["tasks", "list"]), ["Eggs", "Milk"]);
    let replayed = replays(&graph, "L-tasks").await;
    let [.., failed, retried] = replayed.as_slice() else {
        unreachable!("fewer than two replays: {replayed:?}");
    };
    assert_eq!(failed, retried);
    assert_eq!(scope(&env, "tasks:L-tasks")["mode"], "delta");
}

#[tokio::test]
async fn a_list_whose_get_is_404_is_removed_with_its_tasks_and_cursor() {
    let mut env = Env::new();
    let graph = graph_with_tasks(&mut env, Vec::new()).await;
    graph.edit(|data| {
        data.lists.push(list("L-groc", "Groceries", "none"));
        data.tasks
            .insert("L-groc".into(), vec![task("T9", "Eggs", "W/\"g1\"")]);
    });
    env.synced();

    // Deleted, though lists delta hasn't said so: its tasks delta 404s,
    // and so does its GET.
    graph.edit(|data| {
        data.gone_lists.insert("L-groc".into());
        data.fail_delta("L-groc", 404);
    });
    env.synced();

    let lists = env.json(&["lists", "list"]);
    assert_eq!(lists["items"].as_array().map(Vec::len), Some(1));
    env.failure(&["tasks", "complete", "T9"], 3);
    assert_eq!(
        scope(&env, "tasks:L-groc"),
        Value::Null,
        "its cursor is gone"
    );
}

#[tokio::test]
async fn a_list_removed_in_lists_delta_is_tombstoned_and_a_new_one_is_read_whole() {
    let mut env = Env::new();
    let graph = graph_with_tasks(&mut env, Vec::new()).await;
    graph.edit(|data| {
        data.lists.push(list("L-groc", "Groceries", "none"));
        data.tasks
            .insert("L-groc".into(), vec![task("T9", "Eggs", "W/\"g1\"")]);
    });
    env.synced();

    graph.edit(|data| {
        data.lists.retain(|list| list["id"] != "L-groc");
        data.lists.push(list("L-work", "Work", "none"));
        data.tasks
            .insert("L-work".into(), vec![task("T5", "Report", "W/\"w1\"")]);
    });
    env.synced();

    let names: Vec<Value> = env.json(&["lists", "list"])["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|list| list["displayName"].clone())
        .collect();
    assert_eq!(names, [json!("Tasks"), json!("Work")]);
    env.failure(&["tasks", "complete", "T9"], 3);
    assert_eq!(scope(&env, "tasks:L-groc"), Value::Null);
    assert_eq!(
        titles(&env, &["tasks", "list", "--list", "Work"]),
        ["Report"]
    );
    assert_eq!(graph.delta_starts("L-work").await, [None]);

    env.synced();
    assert_eq!(graph.delta_starts("L-work").await.len(), 2);
    assert_eq!(replays(&graph, "L-work").await.len(), 1);
}

#[tokio::test]
async fn the_link_moves_only_after_every_task_it_named_is_hydrated() {
    let mut env = Env::new();
    let graph = graph_with_tasks(
        &mut env,
        vec![
            task("T1", "Milk", "W/\"e1\""),
            task("T2", "Eggs", "W/\"e1\""),
        ],
    )
    .await;
    env.synced();
    graph.edit(|data| {
        let tasks = data.tasks.get_mut("L-tasks").expect("tasks");
        tasks[1]["title"] = json!("Free-range eggs");
        tasks[1]["@odata.etag"] = json!("W/\"e2\"");
        data.forbidden_on_fetch.insert("T2".into());
    });

    let error = env.failure(&["sync", "--wait"], 5);
    assert_eq!(error["error"]["kind"], "rejected");
    let before = replays(&graph, "L-tasks").await;

    graph.edit(|data| data.forbidden_on_fetch.clear());
    env.synced();
    let after = replays(&graph, "L-tasks").await;
    assert_eq!(
        after[before.len()],
        before[before.len() - 1],
        "the failed round's link is replayed"
    );
    assert_eq!(
        titles(&env, &["tasks", "list"]),
        ["Free-range eggs", "Milk"]
    );
    let fetched = graph.batched_gets().await;
    assert!(
        fetched
            .last()
            .is_some_and(|url| url.starts_with("/me/todo/lists/L-tasks/tasks/T2?")),
        "{fetched:?}"
    );
}

#[tokio::test]
async fn a_local_edit_during_a_delta_round_is_not_undone_by_it() {
    let mut env = Env::new();
    let graph = graph_with_tasks(&mut env, vec![task("T1", "Milk", "W/\"e1\"")]).await;
    env.synced();
    Mock::given(method("PATCH"))
        .and(path("/v1.0/me/todo/lists/L-tasks/tasks/T1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(task("T1", "Oat milk", "W/\"e3\"")))
        .mount(&graph.server)
        .await;
    // The phone renamed it first; the delta round that brings that in is
    // slow, and the CLI edit lands while it's in flight.
    graph.edit(|data| {
        let tasks = data.tasks.get_mut("L-tasks").expect("tasks");
        tasks[0]["title"] = json!("Whole milk");
        tasks[0]["@odata.etag"] = json!("W/\"e2\"");
        data.tasks_delay = Some(Duration::from_millis(1500));
    });
    env.json(&["sync"]);
    let started = Instant::now() + Duration::from_secs(5);
    while scope(&env, "tasks:L-tasks")["in_progress"] != true {
        assert!(Instant::now() < started, "the delta round never started");
        std::thread::sleep(Duration::from_millis(20));
    }
    let edited = env.json(&["tasks", "edit", "T1", "--title", "Oat milk"]);
    assert_eq!(edited["items"][0]["title"], "Oat milk");

    env.wait_until_idle();
    assert_eq!(titles(&env, &["tasks", "list"]), ["Oat milk"]);
}
