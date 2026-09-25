//! Rung 5e's `tasks move` through the real binary and daemon, against a
//! fake Graph (docs/blueprint/05-custom-features.md#move-between-lists,
//! docs/blueprint/04-sync-cache.md#instant-local-writes): a move copies the
//! task with everything it holds, checks the copy, and only then deletes
//! the source. A failure before the delete rolls back; an unknown outcome
//! pauses; a crash at any step resumes honestly; and nothing is ever lost.

mod support;

use std::time::Duration;

use serde_json::{Value, json};
use support::Env;
use support::fake_graph::{FakeGraph, list, task};
use support::fake_moves::{create_task, delete_task, put_chunk};
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, ResponseTemplate};

const TASK: &str = r"^/v1\.0/me/todo/lists/L-tasks/tasks/T1$";
const CREATE: &str = r"^/v1\.0/me/todo/lists/L-groc/tasks$";
const COPY: &str = r"^/v1\.0/me/todo/lists/L-groc/tasks/C[0-9]+$";
const COPY_ATTACHMENTS: &str = r"^/v1\.0/me/todo/lists/L-groc/tasks/C[0-9]+/attachments$";
const CHUNKS: &str =
    r"^/v1\.0/users/[^/]+/todo/lists/L-groc/tasks/[^/]+/attachmentSessions/[^/]+/content$";
const SLOW: Duration = Duration::from_secs(30);

/// A small file and one big enough for an upload session (over 3 MiB).
fn small_file() -> Vec<u8> {
    b"S14 small attachment\n".repeat(3)
}

fn big_file() -> Vec<u8> {
    (0..3_500_000_u32).map(|n| (n * 7 % 251) as u8).collect()
}

/// A task with every field set, as Graph gives it (S14).
fn rich_task() -> Value {
    let mut rich = task("T1", "Renew passport", "W/\"t1-1\"");
    let fields = json!({
        "body": { "content": "Photos first\nthen the form", "contentType": "text" },
        "importance": "high",
        "categories": ["Admin"],
        "isReminderOn": true,
        "reminderDateTime": { "dateTime": "2026-09-28T08:30:00.0000000", "timeZone": "UTC" },
        "dueDateTime": { "dateTime": "2026-09-28T00:00:00.0000000", "timeZone": "UTC" },
        "startDateTime": { "dateTime": "2026-09-28T00:00:00.0000000", "timeZone": "UTC" },
        "recurrence": {
            "pattern": { "type": "daily", "interval": 1, "month": 0, "dayOfMonth": 0,
                         "daysOfWeek": [], "firstDayOfWeek": "sunday", "index": "first" },
            "range": { "type": "noEnd", "startDate": "2026-09-28", "endDate": "0001-01-01",
                       "recurrenceTimeZone": "UTC", "numberOfOccurrences": 0 }
        },
        "checklistItems": [
            { "id": "c1", "displayName": "Photos", "isChecked": false, "createdDateTime": "2026-09-24T10:00:00Z" },
            { "id": "c2", "displayName": "Form", "isChecked": true, "createdDateTime": "2026-09-24T10:00:01Z",
              "checkedDateTime": "2026-09-24T11:00:00Z" },
            { "id": "c3", "displayName": "Post it", "isChecked": false, "createdDateTime": "2026-09-24T10:00:02Z" }
        ],
        "linkedResources": [
            { "id": "r1", "webUrl": "https://example.com/passport", "applicationName": "ms-todo",
              "displayName": "Form", "externalId": "passport-1" }
        ]
    });
    if let (Some(rich), Some(fields)) = (rich.as_object_mut(), fields.as_object()) {
        rich.extend(fields.clone());
    }
    rich
}

/// Graph with "Tasks" holding `tasks` and an empty "Groceries", taking a
/// move's requests. T1 carries our extension, from when ms-todo made it.
async fn graph_with(env: &mut Env, tasks: Vec<Value>) -> FakeGraph {
    let graph = FakeGraph::start(
        env,
        vec![
            list("L-tasks", "Tasks", "defaultList"),
            list("L-groc", "Groceries", "none"),
        ],
    )
    .await;
    graph.edit(|data| {
        data.tasks.insert("L-tasks".into(), tasks);
        data.tasks.insert("L-groc".into(), Vec::new());
        data.extensions.insert(
            "T1".into(),
            json!({
                "extensionName": "com.planetaryescape.mstodo",
                "id": "microsoft.graph.openTypeExtension.com.planetaryescape.mstodo",
                "opId": "the-add",
                "assignee": "Sam"
            }),
        );
    });
    graph.accept_moves().await;
    graph
}

/// The rich task, with both files attached, synced.
async fn rich_graph(env: &mut Env) -> FakeGraph {
    let graph = graph_with(env, vec![rich_task()]).await;
    graph.attach("L-tasks", "T1", "small.txt", &small_file());
    graph.attach("L-tasks", "T1", "big.bin", &big_file());
    env.synced();
    graph
}

fn op_id(applied: &Value) -> String {
    applied["op_id"].as_str().expect("op_id").to_owned()
}

fn tasks(env: &Env, list: &str) -> Vec<Value> {
    env.json(&["tasks", "list", "--list", list])["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn titles(env: &Env, list: &str) -> Vec<String> {
    tasks(env, list)
        .iter()
        .filter_map(|task| task["title"].as_str().map(str::to_owned))
        .collect()
}

/// The one copy in Groceries, as Graph holds it.
fn the_copy(graph: &FakeGraph) -> Value {
    let copies = graph.tasks_in("L-groc");
    assert_eq!(copies.len(), 1, "{copies:?}");
    copies[0].clone()
}

/// The DELETEs of tasks in `list` that reached Graph.
async fn deletes_in(graph: &FakeGraph, list: &str) -> usize {
    let prefix = format!("/v1.0/me/todo/lists/{list}/tasks/");
    graph
        .requests("DELETE")
        .await
        .iter()
        .filter(|request| request.url.path().starts_with(&prefix))
        .count()
}

fn move_t1(env: &Env) -> Value {
    env.json(&["tasks", "move", "T1", "--to", "Groceries"])
}

#[tokio::test]
async fn a_move_copies_every_field_and_child_then_deletes_the_source() {
    let mut env = Env::new();
    let graph = rich_graph(&mut env).await;
    let local = env.local_id(&["tasks", "list", "--list", "Tasks"], "T1");

    let plan = env.json(&["tasks", "move", "T1", "--to", "Groceries", "--dry-run"]);
    assert_eq!(plan["action"], "move");
    assert_eq!(plan["list"]["name"], "Groceries");
    assert!(graph.writes().await.is_empty(), "a dry run writes nothing");

    let moved = move_t1(&env);
    assert_eq!(moved["action"], "move");
    // It shows in Groceries at once, keeping its local ID.
    assert_eq!(moved["items"][0]["id"], local.as_str());
    assert_eq!(
        moved["list_ids"][0],
        env.local_id(&["lists", "list"], "L-groc")
    );
    env.settled();
    let op = env.op_in_state(&op_id(&moved), "done");
    assert_eq!(op["action"], "move");

    assert!(graph.task("L-tasks", "T1").is_none(), "the source is gone");
    let copy = the_copy(&graph);
    let copy_id = copy["id"].as_str().expect("id").to_owned();
    for key in [
        "title",
        "body",
        "importance",
        "categories",
        "isReminderOn",
        "reminderDateTime",
        "recurrence",
    ] {
        assert_eq!(copy[key], rich_task()[key], "{key}");
    }
    // A recurring task's dates are written in its recurrence's zone (S14).
    assert_eq!(copy["dueDateTime"]["timeZone"], "UTC");
    assert!(
        copy["dueDateTime"]["dateTime"]
            .as_str()
            .is_some_and(|at| at.starts_with("2026-09-28T00:00"))
    );
    let steps: Vec<(Value, Value)> = copy["checklistItems"]
        .as_array()
        .expect("steps")
        .iter()
        .map(|step| (step["displayName"].clone(), step["isChecked"].clone()))
        .collect();
    assert_eq!(
        steps,
        [
            (json!("Photos"), json!(false)),
            (json!("Form"), json!(true)),
            (json!("Post it"), json!(false))
        ]
    );
    assert_eq!(
        copy["checklistItems"][1]["checkedDateTime"],
        "2026-09-24T11:00:00Z"
    );
    assert_eq!(
        copy["linkedResources"][0]["webUrl"],
        "https://example.com/passport"
    );
    let extension = graph.extension(&copy_id).expect("extension");
    assert_eq!(extension["opId"], op_id(&moved), "the move's own opId");
    assert_eq!(extension["assignee"], "Sam");
    assert_eq!(
        extension["originalCreatedAt"],
        "2026-09-24T10:00:00.1234567Z"
    );
    let mut files = graph.attachments_of(&copy_id);
    files.sort();
    assert_eq!(
        files,
        [
            ("big.bin".to_owned(), big_file()),
            ("small.txt".to_owned(), small_file())
        ]
    );
    // The big one went through an upload session.
    assert!(!graph.requests("PUT").await.is_empty());

    // The cache: the same local task, now the copy, in Groceries only.
    let in_groceries = tasks(&env, "Groceries");
    assert_eq!(in_groceries.len(), 1);
    assert_eq!(in_groceries[0]["id"], local.as_str());
    assert_eq!(in_groceries[0]["graph_id"], copy_id.as_str());
    assert!(tasks(&env, "Tasks").is_empty());
    env.synced();
    assert_eq!(
        tasks(&env, "Groceries").len(),
        1,
        "sync makes no second row"
    );
    assert!(tasks(&env, "Tasks").is_empty());
}

#[tokio::test]
async fn a_rejected_step_before_the_delete_deletes_the_partial_copy_and_keeps_the_source() {
    let mut env = Env::new();
    let graph = rich_graph(&mut env).await;
    let local = env.local_id(&["tasks", "list", "--list", "Tasks"], "T1");
    Mock::given(method("POST"))
        .and(path_regex(
            r"^/v1\.0/me/todo/lists/L-groc/tasks/[^/]+/attachments$",
        ))
        .respond_with(ResponseTemplate::new(400).set_body_json(
            json!({ "error": { "code": "invalidRequest", "message": "no attachments today" } }),
        ))
        .mount(&graph.server)
        .await;

    let moved = move_t1(&env);
    env.settled();
    let op = env.op_in_state(&op_id(&moved), "failed");
    assert!(
        op["last_error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("no attachments today")),
        "{op}"
    );
    assert!(
        graph.tasks_in("L-groc").is_empty(),
        "the partial copy was deleted"
    );
    assert_eq!(deletes_in(&graph, "L-groc").await, 1);
    assert!(
        graph.task("L-tasks", "T1").is_some(),
        "the source is untouched"
    );
    assert_eq!(graph.attachments_of("T1").len(), 2);
    let back = tasks(&env, "Tasks");
    assert_eq!(back.len(), 1);
    assert_eq!(back[0]["id"], local.as_str());
    assert_eq!(back[0]["graph_id"], "T1");
    assert!(tasks(&env, "Groceries").is_empty());
}

#[tokio::test]
async fn a_rejected_create_fails_with_nothing_to_delete() {
    let mut env = Env::new();
    let graph = rich_graph(&mut env).await;
    Mock::given(method("POST"))
        .and(path_regex(CREATE))
        .respond_with(ResponseTemplate::new(403).set_body_json(
            json!({ "error": { "code": "accessDenied", "message": "not your list" } }),
        ))
        .mount(&graph.server)
        .await;
    let moved = move_t1(&env);
    env.settled();
    env.op_in_state(&op_id(&moved), "failed");
    assert_eq!(
        graph.requests("DELETE").await.len(),
        0,
        "nothing was deleted"
    );
    assert_eq!(titles(&env, "Tasks"), ["Renew passport"]);
}

#[tokio::test]
async fn a_copy_that_does_not_match_is_deleted_and_the_source_kept() {
    let mut env = Env::new();
    let graph = rich_graph(&mut env).await;
    // Graph's copy comes back with a step missing.
    let mut wrong = rich_task();
    wrong["id"] = json!("C-wrong");
    wrong["checklistItems"].as_array_mut().expect("steps").pop();
    wrong["extensions"] = json!([]);
    Mock::given(method("GET"))
        .and(path_regex(COPY))
        .respond_with(ResponseTemplate::new(200).set_body_json(wrong))
        .mount(&graph.server)
        .await;
    let moved = move_t1(&env);
    env.settled();
    let op = env.op_in_state(&op_id(&moved), "failed");
    let message = op["last_error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("checklistItems"), "{message}");
    assert!(
        graph.task("L-tasks", "T1").is_some(),
        "the source is never deleted"
    );
    assert_eq!(deletes_in(&graph, "L-tasks").await, 0);
    assert!(graph.tasks_in("L-groc").is_empty(), "the copy was deleted");
}

#[tokio::test]
async fn a_target_list_deleted_meanwhile_undoes_the_move() {
    let mut env = Env::new();
    let graph = rich_graph(&mut env).await;
    // A POST into a deleted list still answers 201; its GET is a 404 (S4).
    graph.edit(|data| {
        data.gone_lists.insert("L-groc".into());
    });
    let moved = move_t1(&env);
    env.settled();
    let op = env.op_in_state(&op_id(&moved), "failed");
    assert!(
        op["last_error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("deleted on another device")),
        "{op}"
    );
    assert!(graph.task("L-tasks", "T1").is_some());
    assert_eq!(deletes_in(&graph, "L-tasks").await, 0);
    assert_eq!(titles(&env, "Tasks"), ["Renew passport"]);
}

#[tokio::test]
async fn a_create_with_no_answer_pauses_and_is_adopted_by_its_op_id() {
    let mut env = Env::new();
    let graph = rich_graph(&mut env).await;
    // Graph makes the copy, and the answer is lost (a 504).
    Mock::given(method("POST"))
        .and(path_regex(CREATE))
        .respond_with(|request: &wiremock::Request| {
            let _ = request;
            ResponseTemplate::new(504)
        })
        .up_to_n_times(1)
        .mount(&graph.server)
        .await;
    let moved = move_t1(&env);
    let op = env.op_in_state(&op_id(&moved), "unknown");
    assert_eq!(op["flagged"], false, "it can still be found by its opId");
    assert!(graph.tasks_in("L-groc").is_empty());
    assert!(
        graph.task("L-tasks", "T1").is_some(),
        "nothing is deleted while paused"
    );
    assert_eq!(graph.requests("DELETE").await.len(), 0);
    // Nothing is sent again by itself.
    env.synced();
    assert_eq!(creates(&graph).await, 1);
}

#[tokio::test]
async fn an_attachment_with_no_answer_pauses_the_move_for_the_user_and_deletes_nothing() {
    let mut env = Env::new();
    let graph = rich_graph(&mut env).await;
    Mock::given(method("POST"))
        .and(path_regex(
            r"^/v1\.0/me/todo/lists/L-groc/tasks/[^/]+/attachments$",
        ))
        .respond_with(ResponseTemplate::new(503))
        .mount(&graph.server)
        .await;
    let moved = move_t1(&env);
    let op = env.op_in_state(&op_id(&moved), "unknown");
    assert_eq!(op["flagged"], true, "only the user can settle it");
    assert!(
        op["note"]
            .as_str()
            .is_some_and(|note| note.contains("outbox retry")),
        "{op}"
    );
    assert!(graph.task("L-tasks", "T1").is_some());
    assert_eq!(
        graph.tasks_in("L-groc").len(),
        1,
        "the partial copy is kept too"
    );
    assert_eq!(graph.requests("DELETE").await.len(), 0);

    // Discarding keeps both: the task goes back to Tasks, and the partial
    // copy arrives in Groceries by sync as a task of its own.
    env.json(&["outbox", "discard", &op_id(&moved), "--yes"]);
    env.synced();
    assert_eq!(titles(&env, "Tasks"), ["Renew passport"]);
    assert_eq!(tasks(&env, "Tasks")[0]["graph_id"], "T1");
    assert_eq!(titles(&env, "Groceries"), ["Renew passport"]);
    assert_eq!(graph.requests("DELETE").await.len(), 0);
}

#[tokio::test]
async fn a_crash_while_creating_resumes_by_finding_the_copy_by_its_op_id() {
    let mut env = Env::new();
    let graph = rich_graph(&mut env).await;
    graph.stall("POST", CREATE, Some(create_task), SLOW).await;
    let moved = move_t1(&env);
    env.op_in_state(&op_id(&moved), "inflight");
    wait_for(|| !graph.tasks_in("L-groc").is_empty());
    env.json(&["daemon", "stop"]);

    // On start it's unknown, never resent; the first sync finds the copy
    // by its opId, and the move carries on from its attachments.
    env.synced();
    env.settled();
    let op = env.op_in_state(&op_id(&moved), "done");
    assert_eq!(op["attempts"], 2);
    assert_eq!(creates(&graph).await, 1, "no second copy");
    assert!(graph.task("L-tasks", "T1").is_none());
    let copy = the_copy(&graph);
    assert_eq!(
        graph.attachments_of(copy["id"].as_str().expect("id")).len(),
        2
    );
    assert_eq!(tasks(&env, "Groceries").len(), 1);
}

#[tokio::test]
async fn a_crash_while_uploading_is_unknown_and_retry_resends_it_by_choice() {
    let mut env = Env::new();
    let graph = rich_graph(&mut env).await;
    // The final chunk of the big file commits it, then the daemon stops.
    graph
        .stall("PUT", CHUNKS, Some(put_chunk), Duration::ZERO)
        .await;
    graph.stall("PUT", CHUNKS, Some(put_chunk), SLOW).await;
    let moved = move_t1(&env);
    wait_for(|| {
        graph
            .tasks_in("L-groc")
            .first()
            .is_some_and(|copy| copy["hasAttachments"] == true)
            && graph
                .attachments_of(the_copy(&graph)["id"].as_str().unwrap_or_default())
                .len()
                == 2
    });
    env.json(&["daemon", "stop"]);
    let op = env.op_in_state(&op_id(&moved), "unknown");
    assert_eq!(op["flagged"], true);
    assert!(graph.task("L-tasks", "T1").is_some());
    assert_eq!(graph.requests("DELETE").await.len(), 0);

    // A retry sends it again, as the user chose: it's attached twice, the
    // copy no longer matches, and the move is undone, the source kept.
    env.json(&["outbox", "retry", &op_id(&moved)]);
    env.settled();
    env.op_in_state(&op_id(&moved), "failed");
    assert!(graph.task("L-tasks", "T1").is_some());
    assert!(graph.tasks_in("L-groc").is_empty());
    assert_eq!(titles(&env, "Tasks"), ["Renew passport"]);
}

#[tokio::test]
async fn a_crash_while_checking_the_copy_resumes_the_check() {
    let mut env = Env::new();
    let graph = rich_graph(&mut env).await;
    graph.stall("GET", COPY_ATTACHMENTS, None, SLOW).await;
    let moved = move_t1(&env);
    until_sent(&graph, "GET", "L-groc/tasks/C", "/attachments").await;
    env.json(&["daemon", "stop"]);
    assert!(
        graph.task("L-tasks", "T1").is_some(),
        "stopped before the delete"
    );
    env.settled();
    env.op_in_state(&op_id(&moved), "done");
    assert!(graph.task("L-tasks", "T1").is_none());
    assert_eq!(graph.tasks_in("L-groc").len(), 1);
}

/// A crash while the source's DELETE is on its way, then `setup` makes
/// Graph hold one of 04's four cases before the daemon starts again.
async fn crash_in_delete(effect: bool, setup: impl FnOnce(&FakeGraph)) -> (Env, FakeGraph, String) {
    let mut env = Env::new();
    let graph = rich_graph(&mut env).await;
    graph
        .stall("DELETE", TASK, effect.then_some(delete_task as _), SLOW)
        .await;
    let moved = move_t1(&env);
    until_sent(&graph, "DELETE", "L-tasks/tasks/T1", "").await;
    env.json(&["daemon", "stop"]);
    setup(&graph);
    (env, graph, op_id(&moved))
}

#[tokio::test]
async fn a_crash_in_the_delete_with_the_source_gone_and_the_copy_there_is_done() {
    let (env, graph, op) = crash_in_delete(true, |_| {}).await;
    env.settled();
    env.op_in_state(&op, "done");
    assert!(graph.task("L-tasks", "T1").is_none());
    assert_eq!(graph.tasks_in("L-groc").len(), 1);
    assert_eq!(titles(&env, "Groceries"), ["Renew passport"]);
}

#[tokio::test]
async fn a_crash_in_the_delete_with_both_there_finishes_the_delete() {
    let (env, graph, op) = crash_in_delete(false, |_| {}).await;
    env.settled();
    env.op_in_state(&op, "done");
    assert!(graph.task("L-tasks", "T1").is_none());
    assert_eq!(graph.tasks_in("L-groc").len(), 1);
}

#[tokio::test]
async fn a_crash_in_the_delete_with_the_copy_gone_rolls_back_and_loses_nothing() {
    let (env, graph, op) = crash_in_delete(false, |graph| {
        graph.edit(|data| {
            data.tasks.insert("L-groc".into(), Vec::new());
        });
    })
    .await;
    env.settled();
    env.op_in_state(&op, "failed");
    assert!(graph.task("L-tasks", "T1").is_some());
    assert_eq!(
        deletes_in(&graph, "L-groc").await,
        0,
        "the copy is never deleted"
    );
    assert_eq!(titles(&env, "Tasks"), ["Renew passport"]);
    assert_eq!(tasks(&env, "Tasks")[0]["graph_id"], "T1");
}

#[tokio::test]
async fn a_crash_in_the_delete_with_both_gone_keeps_the_local_copy_for_the_user() {
    let (env, graph, op) = crash_in_delete(true, |graph| {
        graph.edit(|data| {
            data.tasks.insert("L-groc".into(), Vec::new());
        });
    })
    .await;
    let paused = env.op_in_state(&op, "unknown");
    assert_eq!(paused["flagged"], true);
    assert!(
        paused["note"]
            .as_str()
            .is_some_and(|note| note.contains("both")),
        "{paused}"
    );
    // The whole task is still here.
    assert_eq!(titles(&env, "Groceries"), ["Renew passport"]);
    assert_eq!(creates(&graph).await, 1, "nothing re-created by itself");

    // `outbox retry` re-creates it, attachments and all, in Groceries.
    env.json(&["outbox", "retry", &op]);
    env.settled();
    env.op_in_state(&op, "done");
    assert_eq!(creates(&graph).await, 2);
    let copy = the_copy(&graph);
    assert_eq!(copy["checklistItems"].as_array().map(Vec::len), Some(3));
    let mut files = graph.attachments_of(copy["id"].as_str().expect("id"));
    files.sort();
    assert_eq!(files[1].1, small_file());
    assert_eq!(files[0].1, big_file());
}

#[tokio::test]
async fn undo_moves_it_back_and_refuses_once_it_changed_since() {
    let mut env = Env::new();
    let graph = rich_graph(&mut env).await;
    let local = env.local_id(&["tasks", "list", "--list", "Tasks"], "T1");
    let moved = move_t1(&env);
    env.settled();
    env.op_in_state(&op_id(&moved), "done");

    let undone = env.json(&["undo"]);
    assert_eq!(undone["undoes"], op_id(&moved));
    env.settled();
    env.op_in_state(&op_id(&undone), "done");
    let back = tasks(&env, "Tasks");
    assert_eq!(back.len(), 1);
    assert_eq!(back[0]["id"], local.as_str(), "the same local task");
    assert!(graph.tasks_in("L-groc").is_empty());
    let home = graph.tasks_in("L-tasks");
    assert_eq!(home.len(), 1);
    assert_eq!(
        graph
            .attachments_of(home[0]["id"].as_str().expect("id"))
            .len(),
        2
    );

    // Moved again, then renamed on the phone: undo would overwrite that.
    let again = env.json(&["tasks", "move", &local, "--to", "Groceries"]);
    env.settled();
    env.op_in_state(&op_id(&again), "done");
    let copy_id = the_copy(&graph)["id"].as_str().expect("id").to_owned();
    graph.edit(|data| {
        if let Some(copy) = data
            .tasks
            .get_mut("L-groc")
            .and_then(|tasks| tasks.iter_mut().find(|task| task["id"] == copy_id.as_str()))
        {
            copy["title"] = json!("Renew passport (booked)");
            copy["@odata.etag"] = json!("W/\"phone\"");
        }
    });
    env.synced();
    let error = env.failure(&["undo", &op_id(&again)], 5);
    let message = error["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("changed since (title)"), "{error}");
    assert_eq!(graph.tasks_in("L-groc").len(), 1, "nothing was queued");
}

#[tokio::test]
async fn a_bulk_move_of_three_is_one_command_and_one_undo() {
    let mut env = Env::new();
    let graph = graph_with(
        &mut env,
        vec![
            task("T1", "Milk", "W/\"1\""),
            task("T2", "Eggs", "W/\"2\""),
            task("T3", "Bread", "W/\"3\""),
        ],
    )
    .await;
    env.synced();
    // Off a terminal, moving several needs --yes.
    env.failure(&["tasks", "move", "T1", "T2", "T3", "--to", "Groceries"], 2);
    let moved = env.json(&[
        "tasks",
        "move",
        "T1",
        "T2",
        "T3",
        "--to",
        "Groceries",
        "--yes",
    ]);
    assert_eq!(moved["items"].as_array().map(Vec::len), Some(3));
    env.settled();
    let ops: Vec<Value> = env
        .outbox()
        .into_iter()
        .filter(|op| op["command_id"] == op_id(&moved))
        .collect();
    assert_eq!(ops.len(), 3);
    assert!(ops.iter().all(|op| op["state"] == "done"), "{ops:?}");
    let mut moved_titles = titles(&env, "Groceries");
    moved_titles.sort();
    assert_eq!(moved_titles, ["Bread", "Eggs", "Milk"]);
    assert!(graph.tasks_in("L-tasks").is_empty());

    env.json(&["undo"]);
    env.settled();
    let mut back = titles(&env, "Tasks");
    back.sort();
    assert_eq!(back, ["Bread", "Eggs", "Milk"]);
    assert!(graph.tasks_in("L-groc").is_empty());
    assert_eq!(graph.tasks_in("L-tasks").len(), 3);
}

#[tokio::test]
async fn moving_to_the_list_it_is_in_or_an_unknown_list_is_refused() {
    let mut env = Env::new();
    let graph = graph_with(&mut env, vec![task("T1", "Milk", "W/\"1\"")]).await;
    env.synced();
    env.failure(&["tasks", "move", "T1", "--to", "Tasks"], 2);
    env.failure(&["tasks", "move", "T1", "--to", "Nowhere"], 3);
    assert!(graph.writes().await.is_empty());
}

/// Wait until a `verb` request whose path holds `part` and ends with `end`
/// has reached Graph.
async fn until_sent(graph: &FakeGraph, verb: &str, part: &str, end: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let sent = graph.requests(verb).await.iter().any(|request| {
            let path = request.url.path();
            path.contains(part) && path.ends_with(end)
        });
        if sent {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no {verb} …{part}…{end}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The task creates in Groceries that reached Graph.
async fn creates(graph: &FakeGraph) -> usize {
    graph
        .requests("POST")
        .await
        .iter()
        .filter(|request| request.url.path() == "/v1.0/me/todo/lists/L-groc/tasks")
        .count()
}

fn wait_for(ready: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while !ready() {
        assert!(std::time::Instant::now() < deadline, "never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[tokio::test]
async fn a_move_not_started_yet_is_discarded_back_where_it_was() {
    let mut env = Env::new();
    let graph = rich_graph(&mut env).await;
    let local = env.local_id(&["tasks", "list", "--list", "Tasks"], "T1");
    // Throttled while reading the source: nothing is written, and it waits.
    Mock::given(method("GET"))
        .and(path_regex(TASK))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("Retry-After", "120")
                .set_body_json(
                    json!({ "error": { "code": "TooManyRequests", "message": "slow" } }),
                ),
        )
        .mount(&graph.server)
        .await;
    let moved = move_t1(&env);
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while env.op_in_state(&op_id(&moved), "pending")["last_error"].is_null() {
        assert!(std::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        titles(&env, "Groceries"),
        ["Renew passport"],
        "shown there at once"
    );
    env.json(&["outbox", "discard", &op_id(&moved), "--yes"]);
    let back = tasks(&env, "Tasks");
    assert_eq!(back.len(), 1);
    assert_eq!(back[0]["id"], local.as_str());
    assert_eq!(back[0]["graph_id"], "T1");
    assert!(tasks(&env, "Groceries").is_empty());
    assert!(graph.writes().await.is_empty(), "nothing reached Graph");
}

#[tokio::test]
async fn ids_output_is_one_local_id_per_line() {
    let mut env = Env::new();
    let _graph = graph_with(&mut env, vec![task("T1", "Milk", "W/\"1\"")]).await;
    env.synced();
    let local = env.local_id(&["tasks", "list", "--list", "Tasks"], "T1");
    for dry_run in [true, false] {
        let mut command = env.cmd();
        command.args([
            "--format",
            "ids",
            "tasks",
            "move",
            "T1",
            "--to",
            "Groceries",
        ]);
        if dry_run {
            command.arg("--dry-run");
        }
        let output = command.assert().success().get_output().stdout.clone();
        assert_eq!(String::from_utf8_lossy(&output), format!("{local}\n"));
    }
}
