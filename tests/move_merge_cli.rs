//! D-067's moves through the real binary and daemon, against a fake Graph:
//! `tasks move` picking its tasks by `--overdue` or `--due-before` as
//! `reschedule` does, and `lists merge`, which moves a whole list through
//! the same move job and deletes the emptied list only once every move is
//! done and Microsoft To Do shows it empty.

mod support;

use serde_json::{Value, json};
use support::Env;
use support::fake_graph::{FakeGraph, list, task};

/// Graph with "Tasks", "Groceries" and "Errands", taking moves and list
/// writes.
async fn graph_with(env: &mut Env, tasks: Vec<Value>, errands: Vec<Value>) -> FakeGraph {
    let graph = FakeGraph::start(
        env,
        vec![
            list("L-tasks", "Tasks", "defaultList"),
            list("L-groc", "Groceries", "none"),
            list("L-err", "Errands", "none"),
        ],
    )
    .await;
    graph.edit(|data| {
        data.tasks.insert("L-tasks".into(), tasks);
        data.tasks.insert("L-groc".into(), Vec::new());
        data.tasks.insert("L-err".into(), errands);
    });
    graph.accept_moves().await;
    graph.accept_catalog().await;
    graph.accept_task_patches().await;
    env.synced();
    graph
}

/// A task due on `day`, as Graph gives it back (midnight London as UTC).
fn due(id: &str, title: &str, day: chrono::NaiveDate) -> Value {
    let before = day - chrono::Duration::days(1);
    let mut task = task(id, title, &format!("W/\"{id}\""));
    task["dueDateTime"] = json!({
        "dateTime": format!("{}T23:00:00.0000000", before.format("%Y-%m-%d")),
        "timeZone": "UTC"
    });
    task
}

fn completed(id: &str, title: &str) -> Value {
    let mut task = task(id, title, &format!("W/\"{id}\""));
    task["status"] = json!("completed");
    task
}

fn titles(items: &Value) -> Vec<String> {
    let mut titles: Vec<String> = items
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item["title"].as_str().map(str::to_owned))
        .collect();
    titles.sort();
    titles
}

fn graph_titles(graph: &FakeGraph, list: &str) -> Vec<String> {
    titles(&Value::Array(graph.tasks_in(list)))
}

fn op_id(answer: &Value) -> String {
    answer["op_id"].as_str().expect("op_id").to_owned()
}

#[tokio::test]
async fn overdue_tasks_move_as_one_command_that_one_undo_reverses() {
    let mut env = Env::new();
    let graph = graph_with(&mut env, Vec::new(), Vec::new()).await;
    let today = env.today();
    let day = |days: i64| today + chrono::Duration::days(days);
    let mut done_late = due("T4", "Done late", day(-3));
    done_late["status"] = json!("completed");
    graph.edit(|data| {
        data.tasks.insert(
            "L-tasks".into(),
            vec![
                due("T1", "Renew passport", day(-2)),
                due("T2", "Dentist", day(-1)),
                due("T3", "Bins out", day(0)),
                done_late,
            ],
        );
        data.tasks
            .insert("L-err".into(), vec![due("E1", "Post parcel", day(-1))]);
        // Already where it's going: picked by the rule, and left there.
        data.tasks
            .insert("L-groc".into(), vec![due("G1", "Oat milk", day(-1))]);
    });
    env.synced();

    let plan = env.json(&[
        "tasks",
        "move",
        "--overdue",
        "--to",
        "Groceries",
        "--dry-run",
    ]);
    assert_eq!(plan["action"], "move");
    assert_eq!(
        titles(&plan["targets"]),
        ["Dentist", "Post parcel", "Renew passport"]
    );
    // Off a terminal, several need --yes; nothing is queued without it.
    let refused = env.failure(&["tasks", "move", "--overdue", "--to", "Groceries"], 2);
    assert!(
        refused["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("--yes")),
        "{refused}"
    );

    // --list narrows the pick to one list.
    let one = env.json(&[
        "tasks",
        "move",
        "--due-before",
        "today",
        "--list",
        "Errands",
        "--to",
        "Groceries",
        "--dry-run",
    ]);
    assert_eq!(titles(&one["targets"]), ["Post parcel"]);

    let moved = env.json(&["tasks", "move", "--overdue", "--to", "Groceries", "--yes"]);
    assert_eq!(moved["action"], "move");
    assert_eq!(moved["items"].as_array().map(Vec::len), Some(3));
    env.settled();
    let ops: Vec<Value> = env
        .outbox()
        .into_iter()
        .filter(|op| op["command_id"] == op_id(&moved).as_str())
        .collect();
    assert_eq!(ops.len(), 3, "one operation per task, one op_id");
    assert!(ops.iter().all(|op| op["state"] == "done"), "{ops:?}");
    assert_eq!(
        graph_titles(&graph, "L-groc"),
        ["Dentist", "Oat milk", "Post parcel", "Renew passport"]
    );
    assert_eq!(graph_titles(&graph, "L-tasks"), ["Bins out", "Done late"]);

    // One undo moves all three back where each came from.
    env.json(&["undo", &op_id(&moved)]);
    env.settled();
    assert_eq!(
        graph_titles(&graph, "L-tasks"),
        ["Bins out", "Dentist", "Done late", "Renew passport"]
    );
    assert_eq!(graph_titles(&graph, "L-err"), ["Post parcel"]);
    assert_eq!(graph_titles(&graph, "L-groc"), ["Oat milk"]);
}

#[tokio::test]
async fn named_tasks_and_a_selection_can_not_be_mixed() {
    let mut env = Env::new();
    let _graph = graph_with(&mut env, vec![task("T1", "One", "W/\"1\"")], Vec::new()).await;
    // Clap refuses TASK with --overdue before anything is sent.
    env.cmd()
        .args(["tasks", "move", "T1", "--overdue", "--to", "Groceries"])
        .assert()
        .code(2);
}

#[tokio::test]
async fn a_merge_moves_the_open_tasks_and_keeps_the_list_and_its_completed_ones() {
    let mut env = Env::new();
    let graph = graph_with(
        &mut env,
        Vec::new(),
        vec![
            task("E1", "Post parcel", "W/\"E1\""),
            task("E2", "Return shoes", "W/\"E2\""),
            completed("E3", "Collect keys"),
        ],
    )
    .await;

    let plan = env.json(&[
        "lists",
        "merge",
        "Errands",
        "--into",
        "Groceries",
        "--dry-run",
    ]);
    assert_eq!(plan["action"], "merge_list");
    assert_eq!(plan["list"]["name"], "Groceries");
    assert_eq!(titles(&plan["targets"]), ["Post parcel", "Return shoes"]);
    assert_eq!(plan["lists"][0]["name"], "Errands");
    assert_eq!(plan["lists"][0]["changes"]["completed_left"], 1);
    assert!(graph.writes().await.is_empty(), "a dry run writes nothing");

    let refused = env.failure(&["lists", "merge", "Errands", "--into", "Groceries"], 2);
    assert!(
        refused["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("--yes")),
        "{refused}"
    );
    let merged = env.json(&["lists", "merge", "Errands", "--into", "Groceries", "--yes"]);
    assert_eq!(merged["action"], "merge_list");
    env.settled();
    assert_eq!(
        graph_titles(&graph, "L-groc"),
        ["Post parcel", "Return shoes"]
    );
    assert_eq!(graph_titles(&graph, "L-err"), ["Collect keys"]);
    assert!(graph.list_named("Errands").is_some(), "the list stays");

    // One undo moves them back.
    env.json(&["undo", &op_id(&merged)]);
    env.settled();
    assert_eq!(
        graph_titles(&graph, "L-err"),
        ["Collect keys", "Post parcel", "Return shoes"]
    );
}

#[tokio::test]
async fn delete_source_waits_for_every_move_and_needs_completed_tasks_moved_too() {
    let mut env = Env::new();
    let graph = graph_with(
        &mut env,
        Vec::new(),
        vec![
            task("E1", "Post parcel", "W/\"E1\""),
            completed("E2", "Collect keys"),
        ],
    )
    .await;

    // Deleting it would lose the completed task.
    let refused = env.failure(
        &[
            "lists",
            "merge",
            "Errands",
            "--into",
            "Groceries",
            "--delete-source",
            "--yes",
        ],
        2,
    );
    assert!(
        refused["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("--include-completed")),
        "{refused}"
    );
    assert!(graph.writes().await.is_empty());

    let merged = env.json(&[
        "lists",
        "merge",
        "Errands",
        "--into",
        "Groceries",
        "--include-completed",
        "--delete-source",
        "--yes",
    ]);
    env.settled();
    let delete = env.op_in_state(&format!("{}.delete", op_id(&merged)), "done");
    assert_eq!(delete["action"], "delete_list");
    assert!(graph.list_named("Errands").is_none(), "deleted once empty");
    assert_eq!(
        graph_titles(&graph, "L-groc"),
        ["Collect keys", "Post parcel"]
    );
    let lists = env.json(&["lists", "list"])["items"].clone();
    assert!(
        !lists.to_string().contains("Errands"),
        "gone from the cache: {lists}"
    );

    // Undo the moves before the delete: refused, saying which comes first.
    let first = env.failure(&["undo", &op_id(&merged)], 2);
    assert!(
        first["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains(".delete")),
        "{first}"
    );
    // `undo` with no ID takes the delete first, then the moves.
    env.json(&["undo"]);
    env.settled();
    let remade = graph.list_named("Errands").expect("made again");
    env.json(&["undo"]);
    env.settled();
    let remade_id = remade["id"].as_str().expect("id");
    assert_eq!(
        graph_titles(&graph, remade_id),
        ["Collect keys", "Post parcel"]
    );
    assert!(graph_titles(&graph, "L-groc").is_empty());
}

#[tokio::test]
async fn undoing_a_merge_drops_its_delete_while_that_still_waits() {
    let mut env = Env::new();
    let graph = graph_with(
        &mut env,
        Vec::new(),
        vec![task("E1", "Post parcel", "W/\"E1\"")],
    )
    .await;
    // The delete's emptiness check can't reach Graph, so it waits out a
    // backoff, still pending.
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/v1.0/me/todo/lists/L-err/tasks"))
        .respond_with(wiremock::ResponseTemplate::new(503))
        .with_priority(1)
        .mount(&graph.server)
        .await;
    let merged = env.json(&[
        "lists",
        "merge",
        "Errands",
        "--into",
        "Groceries",
        "--delete-source",
        "--yes",
    ]);
    let delete_id = format!("{}.delete", op_id(&merged));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let waiting = env
            .outbox()
            .into_iter()
            .find(|op| op["op_id"] == delete_id.as_str())
            .expect("the delete");
        if waiting["state"] == "pending" && !waiting["last_error"].is_null() {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "{waiting}");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    env.json(&["undo", &op_id(&merged)]);
    env.settled();
    assert!(
        !env.outbox()
            .iter()
            .any(|op| op["op_id"] == delete_id.as_str()),
        "the waiting delete went with the undo"
    );
    assert!(graph.list_named("Errands").is_some(), "never deleted");
    assert_eq!(graph_titles(&graph, "L-err"), ["Post parcel"]);
    assert!(graph_titles(&graph, "L-groc").is_empty());
}

#[tokio::test]
async fn a_list_graph_still_holds_tasks_in_is_kept() {
    let mut env = Env::new();
    let graph = graph_with(
        &mut env,
        Vec::new(),
        vec![task("E1", "Post parcel", "W/\"E1\"")],
    )
    .await;
    // Added on the phone after the last sync: the cache doesn't know it.
    graph.edit(|data| {
        data.tasks.get_mut("L-err").expect("errands").push(task(
            "E9",
            "From the phone",
            "W/\"E9\"",
        ));
    });
    let merged = env.json(&[
        "lists",
        "merge",
        "Errands",
        "--into",
        "Groceries",
        "--delete-source",
        "--yes",
    ]);
    env.settled();
    let delete = env.op_in_state(&format!("{}.delete", op_id(&merged)), "failed");
    assert!(
        delete["last_error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("still shows 1 task")),
        "{delete}"
    );
    assert!(graph.list_named("Errands").is_some(), "kept");
    assert_eq!(graph_titles(&graph, "L-err"), ["From the phone"]);
    assert_eq!(graph_titles(&graph, "L-groc"), ["Post parcel"]);
}

#[tokio::test]
async fn a_merge_is_refused_into_itself_and_from_a_built_in_list_it_would_delete() {
    let mut env = Env::new();
    let _graph = graph_with(&mut env, Vec::new(), Vec::new()).await;
    env.failure(
        &["lists", "merge", "Errands", "--into", "Errands", "--yes"],
        2,
    );
    env.failure(
        &[
            "lists",
            "merge",
            "Tasks",
            "--into",
            "Errands",
            "--delete-source",
            "--yes",
        ],
        2,
    );
    env.failure(&["lists", "merge", "Nope", "--into", "Errands", "--yes"], 3);
}
