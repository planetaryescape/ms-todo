//! S12 recurrence: completing keeps the series ID open on its next day
//! and makes a completed copy. Each command may complete only the occurrence
//! from which it was planned, even while Graph's answer is delayed.

mod support;

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use support::Env;
use support::fake_graph::{FakeGraph, list, task};
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, Request, ResponseTemplate};

const TASK: &str = "/v1.0/me/todo/lists/L-tasks/tasks/T-r";

async fn daily(env: &mut Env) -> Arc<FakeGraph> {
    let graph =
        Arc::new(FakeGraph::start(env, vec![list("L-tasks", "Tasks", "defaultList")]).await);
    let mut recurring = task("T-r", "Water plants", "W/\"r1\"");
    recurring["dueDateTime"] = json!({ "dateTime": "2026-09-24T00:00:00", "timeZone": "UTC" });
    recurring["recurrence"] = json!({
        "pattern": { "type": "daily", "interval": 1 },
        "range": { "type": "noEnd", "startDate": "2026-09-24", "recurrenceTimeZone": "UTC" }
    });
    graph.edit(|data| {
        data.tasks.insert("L-tasks".into(), vec![recurring]);
    });
    let responding = Arc::clone(&graph);
    Mock::given(method("PATCH"))
        .and(path(TASK))
        .and(body_partial_json(json!({ "status": "completed" })))
        .respond_with(move |request: &Request| {
            let mut response = ResponseTemplate::new(500);
            responding.edit(|data| {
                let tasks = data.tasks.get_mut("L-tasks").expect("list");
                let at = tasks
                    .iter()
                    .position(|task| task["id"] == "T-r")
                    .expect("series");
                let current = &tasks[at];
                if request
                    .headers
                    .get("if-match")
                    .and_then(|value| value.to_str().ok())
                    != current["@odata.etag"].as_str()
                {
                    response = ResponseTemplate::new(412).set_body_json(json!({
                        "error": { "code": "ErrorPreconditionFailed", "message": "etag changed" }
                    }));
                    return;
                }
                let before = current["dueDateTime"]["dateTime"].as_str().expect("due");
                let date =
                    chrono::NaiveDate::parse_from_str(&before[..10], "%Y-%m-%d").expect("day");
                let mut copy = current.clone();
                copy["id"] = json!(format!("copy-{date}"));
                copy["status"] = json!("completed");
                copy["recurrence"] = Value::Null;
                let next = date.succ_opt().expect("next day");
                let current = &mut tasks[at];
                current["dueDateTime"]["dateTime"] = json!(format!("{next}T00:00:00"));
                current["@odata.etag"] = json!(format!("W/\"{next}\""));
                current["status"] = json!("notStarted");
                response = ResponseTemplate::new(200)
                    .set_body_json(current.clone())
                    .set_delay(Duration::from_millis(350));
                tasks.push(copy);
            });
            response
        })
        .mount(&graph.server)
        .await;
    env.synced();
    graph
}

fn state(graph: &FakeGraph) -> Vec<Value> {
    let mut tasks = Vec::new();
    graph.edit(|data| tasks = data.tasks["L-tasks"].clone());
    tasks
}

#[tokio::test]
async fn duplicate_completions_advance_one_occurrence_and_a_later_command_advances_the_next() {
    let mut env = Env::new();
    let graph = daily(&mut env).await;
    env.json(&["tasks", "complete", "T-r"]);
    env.json(&["tasks", "complete", "T-r"]);
    env.settled();
    let tasks = state(&graph);
    assert_eq!(
        tasks.len(),
        2,
        "series plus exactly one completed copy: {tasks:?}"
    );
    assert_eq!(tasks[0]["dueDateTime"]["dateTime"], "2026-09-25T00:00:00");
    assert_eq!(tasks[0]["status"], "notStarted");
    env.json(&["tasks", "complete", "T-r"]);
    env.settled();
    let tasks = state(&graph);
    assert_eq!(tasks.len(), 3, "an intentional next completion is allowed");
    assert_eq!(tasks[0]["dueDateTime"]["dateTime"], "2026-09-26T00:00:00");
}

#[tokio::test]
async fn a_completion_planned_before_an_external_rollover_leaves_the_new_occurrence_alone() {
    let mut env = Env::new();
    let graph = daily(&mut env).await;
    graph.edit(|data| {
        let series = &mut data.tasks.get_mut("L-tasks").expect("list")[0];
        series["dueDateTime"]["dateTime"] = json!("2026-09-25T00:00:00");
        series["@odata.etag"] = json!("W/\"phone\"");
    });
    let completed = env.json(&["tasks", "complete", "T-r"]);
    env.settled();
    let tasks = state(&graph);
    assert_eq!(
        tasks.len(),
        1,
        "the new occurrence must not be completed: {tasks:?}"
    );
    assert_eq!(tasks[0]["dueDateTime"]["dateTime"], "2026-09-25T00:00:00");
    let op = env.op_in_state(completed["op_id"].as_str().expect("op_id"), "failed");
    assert_eq!(op["last_error"]["kind"], "conflict", "{op}");
    assert!(graph.requests("PATCH").await.is_empty());
}
