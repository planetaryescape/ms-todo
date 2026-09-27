//! Defer, Someday and `next` through the real binary and daemon, against a
//! fake Graph (docs/blueprint/05-custom-features.md#defer-and-someday,
//! rung 9a): `deferUntil` and `someday` in our extension, what `tasks
//! list` hides and says it hid, search finding them anyway, undo, and
//! `next`'s order and reasons.

mod support;

use chrono::Duration;
use serde_json::{Value, json};
use support::Env;
use support::fake_graph::{FakeGraph, list, task};

fn with(mut task: Value, extra: Value) -> Value {
    if let (Some(task), Some(extra)) = (task.as_object_mut(), extra.as_object()) {
        task.extend(extra.clone());
    }
    task
}

fn due(day: chrono::NaiveDate) -> Value {
    json!({ "dueDateTime": {
        "dateTime": format!("{}T00:00:00.0000000", day.format("%Y-%m-%d")),
        "timeZone": "Europe/London"
    } })
}

/// Graph with "Tasks" and "Home", taking task writes as Graph does.
async fn graph_with(env: &mut Env, tasks: Vec<Value>) -> FakeGraph {
    let graph = FakeGraph::start(
        env,
        vec![
            list("L-tasks", "Tasks", "defaultList"),
            list("L-home", "Home", "none"),
        ],
    )
    .await;
    graph.edit(|data| {
        data.tasks.insert("L-tasks".into(), tasks);
        data.tasks.insert("L-home".into(), Vec::new());
    });
    graph.accept_task_patches().await;
    graph.accept_moves().await;
    graph
}

fn titles(items: &Value) -> Vec<String> {
    items
        .as_array()
        .expect("items")
        .iter()
        .map(|item| item["title"].as_str().unwrap_or_default().to_owned())
        .collect()
}

/// The Graph copy of the task titled `title` in Tasks.
fn in_graph(graph: &FakeGraph, title: &str) -> Value {
    graph
        .tasks_in("L-tasks")
        .into_iter()
        .find(|task| task["title"] == title)
        .expect("task in Graph")
}

#[tokio::test]
async fn deferred_and_someday_tasks_hide_until_asked_for_and_search_finds_them() {
    let mut env = Env::new();
    let graph = graph_with(&mut env, Vec::new()).await;
    env.synced();
    let today = env.today();
    let tomorrow = (today + Duration::days(1)).format("%Y-%m-%d").to_string();

    // A dry run shows the extension it would write.
    let plan = env.json(&["tasks", "add", "Plan the trip ^tomorrow", "--dry-run"]);
    assert_eq!(plan["changes"]["extensions"][0]["deferUntil"], tomorrow);
    assert_eq!(plan["changes"]["title"], "Plan the trip");

    env.json(&["tasks", "add", "Plan the trip ^tomorrow"]);
    env.json(&["tasks", "add", "Learn the cello +someday"]);
    env.json(&["tasks", "add", "Renew passport", "--due", "today"]);
    env.settled();
    let extension = |title: &str| {
        let id = in_graph(&graph, title)["id"]
            .as_str()
            .expect("id")
            .to_owned();
        graph.extension(&id).expect("our extension")
    };
    assert_eq!(extension("Plan the trip")["deferUntil"], tomorrow);
    assert_eq!(extension("Learn the cello")["someday"], true);
    assert!(
        in_graph(&graph, "Plan the trip")
            .get("dueDateTime")
            .is_none(),
        "a defer never touches the due date"
    );

    let listed = env.json(&["tasks", "list"]);
    assert_eq!(titles(&listed["items"]), ["Renew passport"]);
    assert_eq!(listed["deferred_hidden"], 2);
    assert_eq!(listed["items"][0]["defer_until"], Value::Null);
    assert_eq!(listed["items"][0]["someday"], false);

    let table = env
        .cmd()
        .args(["tasks", "list", "--format", "table"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stderr = String::from_utf8(table.stderr).expect("utf8");
    assert!(
        stderr.contains("2 deferred hidden, --deferred include to show"),
        "{stderr}"
    );

    let included = env.json(&["tasks", "list", "--deferred", "include"]);
    assert_eq!(included["items"].as_array().map(Vec::len), Some(3));
    assert_eq!(included["deferred_hidden"], 0);
    let only = env.json(&["tasks", "list", "--deferred", "only"]);
    assert_eq!(
        titles(&only["items"]),
        ["Plan the trip", "Learn the cello"],
        "by the day they come back, then Someday, from every list"
    );
    assert_eq!(only["items"][0]["defer_until"], tomorrow);
    assert_eq!(only["items"][0]["list"], "Tasks");
    assert_eq!(only["items"][1]["someday"], true);

    // Looking for one is asking for it.
    let found = env.json(&["search", "cello"]);
    assert_eq!(titles(&found["items"]), ["Learn the cello"]);
    assert_eq!(found["items"][0]["someday"], true);
    let searched = env.json(&["tasks", "list", "--search", "trip"]);
    assert_eq!(titles(&searched["items"]), ["Plan the trip"]);
    let marked = env
        .cmd()
        .args(["search", "trip", "--format", "table"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let marked = String::from_utf8(marked).expect("utf8");
    assert!(
        marked.contains(&format!("Plan the trip (deferred to {tomorrow})")),
        "{marked}"
    );

    // Nothing put off is next; what's due today is, with why.
    let next = env.json(&["next"]);
    assert_eq!(titles(&next["items"]), ["Renew passport"]);
    assert_eq!(next["items"][0]["why"], "due today");
    assert_eq!(next["items"][0]["list"], "Tasks");
}

#[tokio::test]
async fn edits_defer_and_park_tasks_and_undo_puts_them_back() {
    let mut env = Env::new();
    let graph = graph_with(&mut env, vec![task("T1", "Plan the trip", "W/\"1\"")]).await;
    env.synced();
    let id = env.local_id(&["tasks", "list"], "T1");
    let later = (env.today() + Duration::days(5))
        .format("%Y-%m-%d")
        .to_string();

    let plan = env.json(&["tasks", "edit", &id, "--defer", &later, "--dry-run"]);
    assert_eq!(plan["changes"], json!({ "deferUntil": later }));

    let deferred = env.json(&["tasks", "edit", &id, "--defer", &later]);
    assert_eq!(deferred["items"][0]["defer_until"], later, "at once");
    assert_eq!(env.json(&["tasks", "list"])["items"], json!([]));
    env.settled();
    assert_eq!(
        graph.extension("T1").expect("extension")["deferUntil"],
        later
    );

    // The same day again queues nothing.
    let again = env.json(&["tasks", "edit", &id, "--defer", &later]);
    assert_eq!(again["items"], json!([]));

    env.json(&["undo"]);
    env.settled();
    assert_eq!(
        titles(&env.json(&["tasks", "list"])["items"]),
        ["Plan the trip"]
    );
    assert!(
        graph
            .extension("T1")
            .is_none_or(|extension| extension.get("deferUntil").is_none()),
        "undo took the field away"
    );

    env.json(&["tasks", "edit", &id, "--someday"]);
    env.settled();
    assert_eq!(graph.extension("T1").expect("extension")["someday"], true);
    assert_eq!(
        titles(&env.json(&["tasks", "list", "--deferred", "only"])["items"]),
        ["Plan the trip"]
    );
    env.json(&["tasks", "edit", &id, "--no-someday"]);
    env.settled();
    assert!(
        graph
            .extension("T1")
            .is_none_or(|extension| extension.get("someday").is_none())
    );
    assert_eq!(
        titles(&env.json(&["tasks", "list"])["items"]),
        ["Plan the trip"]
    );

    // A task whose day has come is back, with nothing written.
    let past = (env.today() - Duration::days(1))
        .format("%Y-%m-%d")
        .to_string();
    env.json(&["tasks", "edit", &id, "--defer", &past]);
    assert_eq!(
        titles(&env.json(&["tasks", "list"])["items"]),
        ["Plan the trip"]
    );
    env.json(&["tasks", "edit", &id, "--clear-defer"]);
    env.settled();
    assert!(
        graph
            .extension("T1")
            .is_none_or(|extension| extension.get("deferUntil").is_none())
    );
}

#[tokio::test]
async fn next_orders_by_urgency_and_says_why() {
    let mut env = Env::new();
    let graph = graph_with(&mut env, Vec::new()).await;
    let today = env.today();
    let created = |task: Value, at: &str| with(task, json!({ "createdDateTime": at }));
    let tasks = vec![
        created(task("T1", "Old idea", "W/\"1\""), "2026-01-01T10:00:00Z"),
        with(
            task("T2", "Soon", "W/\"1\""),
            due(today + Duration::days(2)),
        ),
        with(
            task("T3", "Big one", "W/\"1\""),
            json!({ "importance": "high" }),
        ),
        with(
            task("T4", "Late", "W/\"1\""),
            due(today - Duration::days(3)),
        ),
        with(task("T5", "Today", "W/\"1\""), due(today)),
        with(
            task("T6", "Done", "W/\"1\""),
            with(
                due(today - Duration::days(9)),
                json!({ "status": "completed" }),
            ),
        ),
    ];
    graph.edit(|data| {
        data.tasks.insert("L-tasks".into(), tasks);
    });
    env.synced();

    let next = env.json(&["next"]);
    assert_eq!(
        titles(&next["items"]),
        ["Late", "Today", "Big one", "Soon", "Old idea"]
    );
    let why: Vec<&str> = next["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| item["why"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(why[..4], ["overdue 3d", "due today", "high", "due in 2d"]);
    assert!(why[4].starts_with("added "), "{}", why[4]);

    assert_eq!(
        titles(&env.json(&["next", "--limit", "2"])["items"]),
        ["Late", "Today"]
    );
    assert_eq!(env.json(&["next", "--list", "Home"])["items"], json!([]));
    let csv = env
        .cmd()
        .args(["next", "--limit", "1", "--format", "csv"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let csv = String::from_utf8(csv).expect("utf8");
    assert!(
        csv.starts_with("id,title,list,why,due,importance,sync_state\n"),
        "{csv}"
    );
    assert!(csv.contains(",Late,Tasks,overdue 3d,"), "{csv}");
}
