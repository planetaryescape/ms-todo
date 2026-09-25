//! `tasks list`'s filters and order (rung 8e), and the global flags
//! `--fresh`, `--quiet` and `--no-color`, plus `daemon restart` and
//! `daemon logs`, through the real binary and daemon against a fake Graph.

mod support;

use chrono::{Duration, Local};
use serde_json::{Value, json};
use support::Env;
use support::fake_graph::{FakeGraph, list, task};

/// Midnight UTC of the local day `days` from today, as Graph gives a due
/// date written in UTC: the day itself wherever the tests run.
fn due(days: i64) -> Value {
    let day = Local::now().date_naive() + Duration::days(days);
    json!({ "dateTime": format!("{}T00:00:00.0000000", day.format("%Y-%m-%d")), "timeZone": "UTC" })
}

fn day(days: i64) -> String {
    (Local::now().date_naive() + Duration::days(days))
        .format("%Y-%m-%d")
        .to_string()
}

async fn graph(env: &mut Env) -> FakeGraph {
    let with = |id: &str, title: &str, fields: Value| {
        let mut made = task(id, title, "W/\"e\"");
        if let (Some(made), Some(fields)) = (made.as_object_mut(), fields.as_object()) {
            made.extend(fields.clone());
        }
        made
    };
    let graph = FakeGraph::start(
        env,
        vec![
            list("L-tasks", "Tasks", "defaultList"),
            list("L-work", "Work", "none"),
        ],
    )
    .await;
    graph.edit(|data| {
        data.tasks.insert(
            "L-tasks".into(),
            vec![
                with(
                    "T-late",
                    "Renew passport",
                    json!({ "dueDateTime": due(-3), "importance": "high",
                    "createdDateTime": "2026-09-01T00:00:00Z" }),
                ),
                with(
                    "T-today",
                    "Call mum",
                    json!({ "dueDateTime": due(0), "categories": ["Family"],
                    "createdDateTime": "2026-09-02T00:00:00Z" }),
                ),
                with(
                    "T-none",
                    "Read a book",
                    json!({ "createdDateTime": "2026-09-03T00:00:00Z" }),
                ),
            ],
        );
        data.tasks.insert(
            "L-work".into(),
            vec![
                with(
                    "T-done",
                    "Old report",
                    json!({ "dueDateTime": due(-5), "status": "completed",
                    "createdDateTime": "2026-09-04T00:00:00Z" }),
                ),
                with(
                    "T-wait",
                    "Quarterly plan",
                    json!({ "dueDateTime": due(2),
                    "status": "waitingOnOthers", "importance": "low", "categories": ["family "],
                    "createdDateTime": "2026-09-05T00:00:00Z" }),
                ),
            ],
        );
    });
    env.synced();
    graph
}

fn titles(listed: &Value) -> Vec<String> {
    listed["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|task| task["title"].as_str().unwrap_or_default().to_owned())
        .collect()
}

#[tokio::test]
async fn filters_without_a_list_look_in_every_list_soonest_due_first() {
    let mut env = Env::new();
    let _graph = graph(&mut env).await;

    let overdue = env.json(&["tasks", "list", "--due", "overdue"]);
    assert_eq!(titles(&overdue), ["Renew passport"], "open ones only");
    assert_eq!(overdue["items"][0]["list"], "Tasks");

    let review = env.json(&[
        "tasks",
        "list",
        "--due",
        "before tomorrow",
        "--status",
        "open",
    ]);
    assert_eq!(titles(&review), ["Renew passport", "Call mum"]);
    let everything_due = env.json(&["tasks", "list", "--due", "any"]);
    assert_eq!(
        titles(&everything_due),
        ["Old report", "Renew passport", "Call mum", "Quarterly plan"]
    );
    assert_eq!(
        titles(&env.json(&["tasks", "list", "--due", "today"])),
        ["Call mum"]
    );
    assert_eq!(
        titles(&env.json(&["tasks", "list", "--due", &format!("after {}", day(0))])),
        ["Quarterly plan"]
    );
    assert_eq!(
        titles(&env.json(&["tasks", "list", "--due", "none"])),
        ["Read a book"]
    );
    assert_eq!(
        titles(&env.json(&["tasks", "list", "--completed"])),
        ["Old report"]
    );
    assert_eq!(
        titles(&env.json(&["tasks", "list", "--status", "waiting"])),
        ["Quarterly plan"]
    );
    assert_eq!(
        titles(&env.json(&["tasks", "list", "--category", "FAMILY", "--sort", "title"])),
        ["Call mum", "Quarterly plan"]
    );
    assert_eq!(
        titles(&env.json(&["tasks", "list", "--importance", "p1"])),
        ["Renew passport"]
    );

    // With --list, one list, in its order unless sorted; then the limit.
    let work = env.json(&[
        "tasks", "list", "--list", "Work", "--sort", "due", "--limit", "1",
    ]);
    assert_eq!(titles(&work), ["Old report"]);
    assert!(work["items"][0].get("list").is_none());
    let newest = env.json(&["tasks", "list", "--sort", "created", "--limit", "2"]);
    assert_eq!(
        titles(&newest),
        ["Read a book", "Call mum"],
        "the default list, sorted"
    );

    // A usage error, from clap: exit 2, naming what wasn't understood.
    let output = env
        .cmd()
        .args(["tasks", "list", "--due", "soonish"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    assert!(String::from_utf8_lossy(&output.stderr).contains("soonish"));
}

#[tokio::test]
async fn every_list_output_names_the_list_in_table_and_csv() {
    let mut env = Env::new();
    let _graph = graph(&mut env).await;
    let output = |format: &str| {
        let bytes = env
            .cmd()
            .args([
                "tasks", "list", "--due", "any", "--status", "open", "--format", format,
            ])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        String::from_utf8(bytes).expect("utf8")
    };
    let csv = output("csv");
    let header = csv.lines().next().unwrap_or_default();
    assert!(header.ends_with(",sync_state,list"), "{header}");
    assert!(
        csv.contains("Quarterly plan") && csv.contains(",Work"),
        "{csv}"
    );
    let table = output("table");
    assert!(
        table.lines().next().unwrap_or_default().contains("LIST"),
        "{table}"
    );
}

#[tokio::test]
async fn fresh_syncs_first_and_quiet_keeps_stderr_empty() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    graph.edit(|data| {
        if let Some(tasks) = data.tasks.get_mut("L-work") {
            tasks.push(task("T-new", "Brand new", "W/\"n\""));
        }
    });
    let fresh = env.json(&["--fresh", "tasks", "list", "--list", "Work"]);
    assert!(titles(&fresh).contains(&"Brand new".to_owned()), "{fresh}");

    let output = env
        .cmd()
        .args([
            "--quiet",
            "tasks",
            "add",
            "Quiet one",
            "--no-parse",
            "--start",
            "tomorrow",
        ])
        .args(["--format", "table", "--no-color"])
        .assert()
        .success()
        .get_output()
        .clone();
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn the_daemon_restarts_and_its_log_can_be_read() {
    let mut env = Env::new();
    let _graph = graph(&mut env).await;
    let before = env.json(&["daemon", "status"])["pid"].clone();
    let restarted = env.json(&["daemon", "restart"]);
    assert_eq!(restarted["running"], true);
    assert_ne!(restarted["pid"], before);
    let logs = env.json(&["daemon", "logs"]);
    assert!(
        logs["path"]
            .as_str()
            .unwrap_or_default()
            .ends_with("daemon.log")
    );
    assert!(logs["lines"].is_array());
}
