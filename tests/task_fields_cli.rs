//! The task fields rung 8e added to `tasks add` and `tasks edit`, through
//! the real binary and daemon against a fake Graph: start dates (sent with
//! the due date, S11), recurrences (with `recurrenceTimeZone`, S12),
//! categories and notes from a file; and `tasks show`.

mod support;

use std::io::Write;

use serde_json::{Value, json};
use support::Env;
use support::fake_graph::{FakeGraph, list, task};

const ZONE: &str = "Europe/London";

async fn graph(env: &mut Env) -> FakeGraph {
    let mut due = task("T-due", "Pay rent", "W/\"d\"");
    due["dueDateTime"] = json!({ "dateTime": "2026-10-05T00:00:00.0000000", "timeZone": "UTC" });
    let graph = FakeGraph::start(env, vec![list("L-tasks", "Tasks", "defaultList")]).await;
    graph.edit(|data| {
        data.tasks.insert(
            "L-tasks".into(),
            vec![due, task("T-plain", "Water plants", "W/\"p\"")],
        );
    });
    graph.accept_moves().await;
    graph.accept_task_patches().await;
    env.synced();
    graph
}

/// The body of each write Graph got, in order.
async fn bodies(graph: &FakeGraph) -> Vec<Value> {
    graph
        .writes()
        .await
        .iter()
        .map(|request| serde_json::from_slice(&request.body).unwrap_or_default())
        .collect()
}

fn midnight(day: &str) -> Value {
    json!({ "dateTime": format!("{day}T00:00:00"), "timeZone": ZONE })
}

#[tokio::test]
async fn add_takes_start_categories_a_recurrence_and_notes_from_a_file() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    let mut notes = tempfile::NamedTempFile::new().expect("temp file");
    writeln!(notes, "Line one\nLine two").expect("write");

    let added = env.json(&[
        "tasks",
        "add",
        "Service the boiler",
        "--no-parse",
        "--due",
        "2026-10-01",
        "--start",
        "2026-10-01",
        "--category",
        "Home",
        "--category",
        "Bills",
        "--recur",
        "every year",
        "--body-file",
        notes.path().to_str().expect("path"),
    ]);
    assert_eq!(added["action"], "add");
    env.settled();
    let sent = bodies(&graph).await.pop().expect("a create");
    assert_eq!(sent["startDateTime"], midnight("2026-10-01"));
    assert_eq!(sent["dueDateTime"], midnight("2026-10-01"));
    assert_eq!(sent["categories"], json!(["Home", "Bills"]));
    assert_eq!(sent["recurrence"]["pattern"]["type"], "absoluteYearly");
    assert_eq!(sent["recurrence"]["range"]["startDate"], "2026-10-01");
    assert_eq!(sent["recurrence"]["range"]["recurrenceTimeZone"], ZONE);
    assert_eq!(sent["body"]["content"], "Line one\nLine two\n");
    // Graph counts a recurrence from the start date (D-058): an earlier
    // one would move the due date, so it's refused.
    let error = env.failure(
        &[
            "tasks",
            "add",
            "Gutters",
            "--no-parse",
            "--due",
            "2026-10-05",
            "--start",
            "2026-10-01",
            "--recur",
            "every mon",
        ],
        2,
    );
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("first due"),
        "{error}"
    );

    // A start date alone makes the due date too, and says so.
    let output = env
        .cmd()
        .args([
            "tasks",
            "add",
            "Paint shed",
            "--no-parse",
            "--start",
            "2026-10-03",
        ])
        .args(["--format", "csv"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("sets the due date"), "{stderr}");
    env.settled();
    let sent = bodies(&graph).await.pop().expect("a create");
    assert_eq!(sent["dueDateTime"], midnight("2026-10-03"));

    // Quick add: flags win over the text, with a note.
    let added = env.json(&[
        "tasks",
        "add",
        "Gym @health every mon",
        "--category",
        "Fitness",
        "--recur",
        "every fri",
        "--dry-run",
    ]);
    let changes = &added["changes"];
    assert_eq!(changes["categories"], json!(["Fitness"]));
    assert_eq!(
        changes["recurrence"]["pattern"]["daysOfWeek"],
        json!(["friday"])
    );
    assert_eq!(
        changes["dueDateTime"]["dateTime"]
            .as_str()
            .map(|at| &at[..10]),
        changes["recurrence"]["range"]["startDate"].as_str(),
        "its first time is the due date"
    );
    let error = env.failure(
        &["tasks", "add", "x", "--no-parse", "--recur", "soonish"],
        2,
    );
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("--recur")
    );
    let error = env.failure(&["tasks", "add", "Call #Nowhere", "--strict"], 2);
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("--strict")
    );
}

#[tokio::test]
async fn edit_sets_and_clears_start_recurrence_and_categories_and_undo_puts_them_back() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;

    // The task's own due date goes with a new start date (S11).
    let edited = env.json(&[
        "tasks",
        "edit",
        "Pay rent",
        "--list",
        "Tasks",
        "--start",
        "2026-10-02",
    ]);
    assert_eq!(edited["action"], "edit");
    env.settled();
    let sent = bodies(&graph).await.pop().expect("a patch");
    assert_eq!(sent["startDateTime"], midnight("2026-10-02"));
    // As Graph gave it: midnight in its own zone is sent back as it came.
    assert_eq!(
        sent["dueDateTime"],
        json!({ "dateTime": "2026-10-05T00:00:00.0000000", "timeZone": "UTC" }),
        "kept, not moved to the start"
    );

    let plan = env.json(&[
        "tasks",
        "edit",
        "Water plants",
        "--list",
        "Tasks",
        "--recur",
        "every mon",
        "--due",
        "2026-10-05",
        "--category",
        "Garden",
        "--dry-run",
    ]);
    assert_eq!(
        plan["changes"]["recurrence"]["range"]["recurrenceTimeZone"],
        ZONE
    );
    assert_eq!(
        plan["changes"]["recurrence"]["range"]["startDate"],
        "2026-10-05"
    );
    assert_eq!(plan["changes"]["dueDateTime"], midnight("2026-10-05"));
    assert_eq!(plan["changes"]["categories"], json!(["Garden"]));

    env.json(&[
        "tasks",
        "edit",
        "Water plants",
        "--list",
        "Tasks",
        "--recur",
        "every mon",
        "--category",
        "Garden",
    ]);
    env.settled();
    let sent = bodies(&graph).await.pop().expect("a patch");
    let start = sent["recurrence"]["range"]["startDate"]
        .as_str()
        .expect("start");
    assert_eq!(
        sent["dueDateTime"],
        midnight(start),
        "due on its first Monday"
    );
    // Graph gives a task with a start date a due date (S11).
    let error = env.failure(
        &[
            "tasks",
            "edit",
            "Pay rent",
            "--list",
            "Tasks",
            "--start",
            "tomorrow",
            "--clear-due",
        ],
        2,
    );
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("clearing the due date"),
        "{error}"
    );
    // A repeating task keeps no start date of its own (D-058).
    let error = env.failure(
        &[
            "tasks",
            "edit",
            "Water plants",
            "--list",
            "Tasks",
            "--start",
            "tomorrow",
        ],
        2,
    );
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("repeats"),
        "{error}"
    );
    let shown = env.json(&["tasks", "show", "Water plants", "--list", "Tasks"]);
    assert_eq!(shown["recurrence"]["pattern"]["type"], "weekly");
    assert_eq!(shown["categories"], json!(["Garden"]));

    env.json(&[
        "tasks",
        "edit",
        "Water plants",
        "--list",
        "Tasks",
        "--clear-recur",
        "--clear-categories",
    ]);
    env.settled();
    let sent = bodies(&graph).await.pop().expect("a patch");
    assert_eq!(sent, json!({ "recurrence": null, "categories": [] }));

    // Undo puts back what the last edit changed.
    env.json(&["undo"]);
    env.settled();
    let sent = bodies(&graph).await.pop().expect("a patch");
    assert_eq!(sent["categories"], json!(["Garden"]));
    assert_eq!(sent["recurrence"]["pattern"]["type"], "weekly");

    let error = env.failure(
        &[
            "tasks",
            "edit",
            "Water plants",
            "--list",
            "Tasks",
            "--recur",
            "every mon",
            "--due",
            "-",
        ],
        2,
    );
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("due date")
    );
    env.json(&[
        "tasks",
        "edit",
        "Pay rent",
        "--list",
        "Tasks",
        "--clear-start",
    ]);
    env.settled();
    assert_eq!(
        bodies(&graph).await.pop(),
        Some(json!({ "startDateTime": null }))
    );
}

#[tokio::test]
async fn show_prints_one_task_in_every_format() {
    let mut env = Env::new();
    let _graph = graph(&mut env).await;
    let shown = env.json(&["tasks", "show", "Pay rent", "--list", "Tasks"]);
    assert_eq!(shown["title"], "Pay rent");
    assert_eq!(shown["schema_version"], 2);
    let table = env
        .cmd()
        .args([
            "tasks", "show", "Pay rent", "--list", "Tasks", "--format", "table",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let table = String::from_utf8(table).expect("utf8");
    assert!(
        table.contains("Due") && table.contains("2026-10-05"),
        "{table}"
    );
    let csv = env
        .cmd()
        .args([
            "tasks", "show", "Pay rent", "--list", "Tasks", "--format", "csv",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(
        String::from_utf8(csv)
            .expect("utf8")
            .starts_with("id,title,status")
    );
    env.failure(&["tasks", "show", "nope"], 3);
}
