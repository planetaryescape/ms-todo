//! A new due date on a recurring task moves it without splitting it
//! (S20, D-069): against a fake Graph that splits a recurring task when a
//! due date is written alone, as Graph does, `tasks edit --due` and
//! `reschedule` leave one task, due on the new day, still recurring from
//! it, and one `undo` puts it back.

mod support;

use serde_json::{Value, json};
use support::Env;
use support::fake_graph::{FakeGraph, list, task};

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
