//! A new due date on a recurring task moves it without splitting it
//! (S20, D-069): against a fake Graph that splits a recurring task when a
//! due date is written alone, as Graph does, `tasks edit --due` and
//! `reschedule` leave one task, due on the new day, still recurring from
//! it, and one `undo` puts it back.

mod support;

use serde_json::{Value, json};
use support::Env;
use support::fake_graph::{FakeGraph, list, task};
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, ResponseTemplate};

const TASK: &str = "/v1.0/me/todo/lists/L-tasks/tasks/T-r";

async fn graph_with_daily(env: &mut Env, due: &str) -> FakeGraph {
    let graph = FakeGraph::start(env, vec![list("L-tasks", "Tasks", "defaultList")]).await;
    let mut daily = task("T-r", "Water plants", "W/\"r1\"");
    daily["dueDateTime"] = json!({ "dateTime": due, "timeZone": "UTC" });
    daily["recurrence"] = json!({
        "pattern": { "type": "daily", "interval": 1 },
        "range": { "type": "noEnd", "startDate": "2026-09-20", "recurrenceTimeZone": "UTC" }
    });
    graph.edit(|data| {
        data.tasks.insert("L-tasks".into(), vec![daily]);
    });
    graph.accept_task_patches().await;
    graph
}

/// Every task Graph holds, as (id, due day, recurrence's start).
fn on_graph(graph: &FakeGraph) -> Vec<(String, String, Value)> {
    let mut tasks = Vec::new();
    graph.edit(|data| tasks = data.tasks["L-tasks"].clone());
    tasks
        .iter()
        .map(|task| {
            (
                task["id"].as_str().unwrap_or_default().to_owned(),
                task["dueDateTime"]["dateTime"]
                    .as_str()
                    .unwrap_or_default()
                    .chars()
                    .take(10)
                    .collect(),
                task["recurrence"]["range"]["startDate"].clone(),
            )
        })
        .collect()
}

#[tokio::test]
async fn editing_a_recurring_task_s_due_date_moves_the_series_and_undo_moves_it_back() {
    let mut env = Env::new();
    let graph = graph_with_daily(&mut env, "2026-09-20T00:00:00.0000000").await;
    env.synced();

    let edited = env.json(&["tasks", "edit", "T-r", "--due", "2026-10-05"]);
    env.settled();

    assert_eq!(
        on_graph(&graph),
        [(
            "T-r".to_owned(),
            "2026-10-05".to_owned(),
            json!("2026-10-05")
        )],
        "one task, recurring from its new day"
    );
    env.json(&["undo", edited["op_id"].as_str().expect("op_id")]);
    env.settled();
    let back = on_graph(&graph);
    assert_eq!(back.len(), 1, "{back:?}");
    assert_eq!(back[0].1, "2026-09-20");
}

#[tokio::test]
async fn rescheduling_an_overdue_recurring_task_leaves_one_task() {
    let mut env = Env::new();
    let graph = graph_with_daily(&mut env, "2026-09-20T00:00:00.0000000").await;
    env.synced();

    env.json(&["reschedule", "--overdue", "--to", "2099-01-05", "--yes"]);
    env.settled();

    assert_eq!(
        on_graph(&graph),
        [(
            "T-r".to_owned(),
            "2099-01-05".to_owned(),
            json!("2099-01-05")
        )]
    );
}

#[tokio::test]
async fn a_recurrence_changed_on_another_device_is_kept_not_the_cached_one() {
    let mut env = Env::new();
    let graph = graph_with_daily(&mut env, "2026-09-20T00:00:00.0000000").await;
    env.synced();
    // The phone makes it every two days; ms-todo hasn't synced that.
    graph.edit(|data| {
        let task = &mut data.tasks.get_mut("L-tasks").expect("list")[0];
        task["recurrence"]["pattern"]["interval"] = json!(2);
        task["@odata.etag"] = json!("W/\"phone\"");
    });

    env.json(&["tasks", "edit", "T-r", "--due", "2026-10-05"]);
    env.settled();

    let mut held = Vec::new();
    graph.edit(|data| held = data.tasks["L-tasks"].clone());
    assert_eq!(held.len(), 1, "{held:?}");
    assert_eq!(
        held[0]["recurrence"]["pattern"]["interval"], 2,
        "the phone's"
    );
    assert_eq!(held[0]["recurrence"]["range"]["startDate"], "2026-10-05");
}

#[tokio::test]
async fn a_retry_after_the_recurrence_was_refused_sets_it_from_the_operation() {
    let mut env = Env::new();
    let graph = graph_with_daily(&mut env, "2026-09-20T00:00:00.0000000").await;
    // The second PATCH is refused once, after the first has landed.
    Mock::given(method("PATCH"))
        .and(path(TASK))
        .and(body_partial_json(
            json!({ "recurrence": { "pattern": {} } }),
        ))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": { "code": "invalidRequest", "message": "not now" }
        })))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&graph.server)
        .await;
    env.synced();

    let edited = env.json(&["tasks", "edit", "T-r", "--due", "2026-10-05"]);
    let op_id = edited["op_id"].as_str().expect("op_id").to_owned();
    env.op_in_state(&op_id, "failed");
    // Sync now reads it without a recurrence, as Graph has it.
    env.synced();
    assert!(on_graph(&graph)[0].2.is_null(), "{:?}", on_graph(&graph));

    env.json(&["outbox", "retry", &op_id]);
    env.op_in_state(&op_id, "done");

    assert_eq!(
        on_graph(&graph),
        [(
            "T-r".to_owned(),
            "2026-10-05".to_owned(),
            json!("2026-10-05")
        )]
    );
}

#[tokio::test]
async fn undo_accepts_a_date_graph_moved_to_the_pattern_s_next_day() {
    let mut env = Env::new();
    let graph = graph_with_daily(&mut env, "2026-09-20T00:00:00.0000000").await;
    // As Graph does for a Sunday task given a Wednesday: the recurrence
    // puts it on the next Sunday.
    let sunday = sunday_held();
    Mock::given(method("PATCH"))
        .and(path(TASK))
        .and(body_partial_json(
            json!({ "recurrence": { "pattern": {} } }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(sunday))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&graph.server)
        .await;
    env.synced();

    let edited = env.json(&["tasks", "edit", "T-r", "--due", "2026-09-30"]);
    env.settled();
    // What Graph holds after the answer above (the mock doesn't write it).
    graph.edit(|data| {
        data.tasks.insert("L-tasks".into(), vec![sunday_held()]);
    });
    env.synced();
    assert_eq!(
        env.json(&["tasks", "show", "T-r", "--list", "Tasks"])["dueDateTime"]["dateTime"],
        "2026-10-04T00:00:00.0000000"
    );

    let undone = env.json(&["undo", edited["op_id"].as_str().expect("op_id")]);
    env.settled();

    assert!(
        undone["refused"].as_array().is_none_or(Vec::is_empty),
        "{undone}"
    );
    assert_eq!(on_graph(&graph)[0].1, "2026-09-20");
}

/// The daily task as Graph holds it moved to 4 October.
fn sunday_held() -> Value {
    let mut sunday = task("T-r", "Water plants", "W/\"r9\"");
    sunday["dueDateTime"] = json!({ "dateTime": "2026-10-04T00:00:00.0000000", "timeZone": "UTC" });
    sunday["recurrence"] = json!({
        "pattern": { "type": "daily", "interval": 1 },
        "range": { "type": "noEnd", "startDate": "2026-10-04", "recurrenceTimeZone": "UTC" }
    });
    sunday
}
