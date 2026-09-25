//! Rung 5d through the real binary and daemon, against a fake Graph:
//! `done` places each completion on a local day and filters by phrases,
//! lists and folders; `reschedule` and a bulk `tasks edit` change many
//! tasks as one command that one `undo` reverses, leaving alone any task
//! changed since.

mod support;

use serde_json::{Value, json};
use support::Env;
use support::fake_graph::{FakeGraph, list, task};

/// Graph with "Tasks" and "Groceries" holding these tasks, each task's
/// PATCH answered as Graph would.
async fn graph_with(env: &mut Env, tasks: Vec<Value>, groceries: Vec<Value>) -> FakeGraph {
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
        data.tasks.insert("L-groc".into(), groceries);
    });
    graph.accept_task_patches().await;
    graph
}

fn completed(id: &str, title: &str, at: &str) -> Value {
    let mut task = task(id, title, &format!("W/\"{id}\""));
    task["status"] = json!("completed");
    task["completedDateTime"] = json!({ "dateTime": format!("{at}.0000000"), "timeZone": "UTC" });
    task
}

/// A task due on `day`, as Graph gives it back: midnight London as a UTC
/// instant, 23:00 the day before (S11). In winter that's 23:00 London the
/// day before, which still rounds to `day`.
fn due(id: &str, title: &str, day: &str) -> Value {
    let before = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").expect("day")
        - chrono::Duration::days(1);
    let mut task = task(id, title, &format!("W/\"{id}\""));
    task["dueDateTime"] = json!({
        "dateTime": format!("{}T23:00:00.0000000", before.format("%Y-%m-%d")),
        "timeZone": "UTC"
    });
    task
}

/// Today in the CLI's zone (Europe/London), from the CLI itself: the day
/// `--due today` resolves to. `days` later (negative: earlier).
fn london_day(env: &Env, days: i64) -> String {
    let plan = env.json(&["tasks", "add", "x", "--due", "today", "--dry-run"]);
    let today = plan["changes"]["dueDateTime"]["dateTime"]
        .as_str()
        .and_then(|at| chrono::NaiveDate::parse_from_str(&at[..10], "%Y-%m-%d").ok())
        .expect("today");
    (today + chrono::Duration::days(days))
        .format("%Y-%m-%d")
        .to_string()
}

fn titles(answer: &Value) -> Vec<String> {
    answer["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["title"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn field<'a>(answer: &'a Value, key: &str) -> Vec<&'a str> {
    answer["items"]
        .as_array()
        .map(|items| items.iter().filter_map(|item| item[key].as_str()).collect())
        .unwrap_or_default()
}

/// A task's due date as the CLI reads it: the local day, from CSV.
fn due_of(env: &Env, list: &str, title: &str) -> String {
    let csv = env
        .cmd()
        .args(["tasks", "list", "--list", list, "--format", "csv"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let csv = String::from_utf8(csv).expect("utf8");
    // id,title,status,importance,due,…; the titles here have no commas.
    csv.lines()
        .map(|line| line.split(',').collect::<Vec<_>>())
        .find(|cells| cells.get(1) == Some(&title))
        .and_then(|cells| cells.get(4).map(|due| (*due).to_owned()))
        .unwrap_or_default()
}

#[tokio::test]
async fn done_puts_each_completion_on_its_london_day_newest_first() {
    let mut env = Env::new();
    let _graph = graph_with(
        &mut env,
        vec![
            // Midnight in London in summer, which is 23:00 UTC the day
            // before: truncating would say the 9th.
            completed("T1", "Late one", "2026-09-09T23:00:00"),
            // What Graph writes (S12): 00:00 UTC is 01:00 in London.
            completed("T2", "Midnight UTC", "2026-09-10T00:00:00"),
            completed("T3", "Earlier", "2026-09-08T00:00:00"),
            task("T4", "Still open", "W/\"T4\""),
        ],
        vec![completed("G1", "Oat milk", "2026-09-10T00:00:00")],
    )
    .await;
    env.synced();

    let tenth = env.json(&["done", "--since", "2026-09-10", "--until", "2026-09-10"]);
    assert_eq!(tenth["sync"]["state"], "ready");
    let mut shown = titles(&tenth);
    shown.sort();
    assert_eq!(shown, ["Late one", "Midnight UTC", "Oat milk"]);
    assert_eq!(field(&tenth, "completed_on"), ["2026-09-10"; 3]);
    assert!(field(&tenth, "list").contains(&"Groceries"));

    let before = env.json(&["done", "--since", "2026-09-08", "--until", "2026-09-09"]);
    assert_eq!(titles(&before), ["Earlier"]);

    let week = env.json(&["done", "--since", "2026-09-01", "--list", "Tasks"]);
    assert_eq!(
        field(&week, "completed_on")[2],
        "2026-09-08",
        "newest first"
    );
    assert_eq!(week["items"].as_array().map(Vec::len), Some(3));
    let one = env.json(&["done", "--since", "2026-09-01", "--limit", "1"]);
    assert_eq!(one["items"].as_array().map(Vec::len), Some(1));
    assert_eq!(field(&one, "completed_on"), ["2026-09-10"]);

    // CSV has the day, and the table groups by it.
    let csv = env
        .cmd()
        .args([
            "done",
            "--since",
            "2026-09-08",
            "--list",
            "Tasks",
            "--format",
            "csv",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let csv = String::from_utf8(csv).expect("utf8");
    let mut lines = csv.lines();
    assert_eq!(
        lines.next(),
        Some("id,title,list,completed_on,due,importance,sync_state")
    );
    assert!(csv.contains(",Earlier,Tasks,2026-09-08,"), "{csv}");
    let table = env
        .cmd()
        .args([
            "done",
            "--since",
            "2026-09-08",
            "--until",
            "2026-09-10",
            "--format",
            "table",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let table = String::from_utf8(table).expect("utf8");
    let headings: Vec<&str> = table
        .lines()
        .filter(|line| !line.starts_with(' '))
        .collect();
    assert_eq!(headings, ["Thu 10 Sep", "Tue 8 Sep"], "{table}");

    // A folder's lists only.
    env.json(&["lists", "move", "Groceries", "--folder", "Areas"]);
    let areas = env.json(&["done", "--since", "2026-09-01", "--folder", "Areas"]);
    assert_eq!(titles(&areas), ["Oat milk"]);

    let backwards = env.failure(
        &["done", "--since", "2026-09-10", "--until", "2026-09-01"],
        2,
    );
    assert_eq!(backwards["error"]["kind"], "invalid_input");
}

#[tokio::test]
async fn done_reads_since_as_a_day_looking_back_and_shows_a_completion_just_made() {
    let mut env = Env::new();
    let graph = graph_with(&mut env, Vec::new(), Vec::new()).await;
    env.synced();
    let (yesterday, three_ago) = (london_day(&env, -1), london_day(&env, -3));
    graph.edit(|data| {
        data.tasks.insert(
            "L-tasks".into(),
            vec![
                completed("T1", "Yesterday's", &format!("{yesterday}T00:00:00")),
                completed("T2", "Three days ago", &format!("{three_ago}T00:00:00")),
                task("T3", "Finish today", "W/\"T3\""),
            ],
        );
    });
    env.synced();

    assert_eq!(
        titles(&env.json(&["done", "--since", "yesterday"])),
        ["Yesterday's"]
    );
    assert_eq!(
        titles(&env.json(&["done", "--since", "3 days ago", "--until", "yesterday"])),
        ["Yesterday's", "Three days ago"]
    );
    // By default, the last 7 days.
    assert_eq!(titles(&env.json(&["done"])).len(), 2);

    let t3 = env.local_id(&["tasks", "list"], "T3");
    env.json(&["tasks", "complete", &t3]);
    env.settled();
    let today = env.json(&["done", "--since", "today"]);
    assert_eq!(titles(&today), ["Finish today"]);
    assert_eq!(field(&today, "completed_on"), [london_day(&env, 0)]);
}

#[tokio::test]
async fn reschedule_overdue_previews_asks_and_one_undo_reverses_the_batch_but_a_task_changed_since()
{
    let mut env = Env::new();
    let graph = graph_with(&mut env, Vec::new(), Vec::new()).await;
    env.synced();
    let (two_ago, yesterday, today, tomorrow) = (
        london_day(&env, -2),
        london_day(&env, -1),
        london_day(&env, 0),
        london_day(&env, 1),
    );
    let mut done_late = due("T4", "Done late", &two_ago);
    done_late["status"] = json!("completed");
    graph.edit(|data| {
        data.tasks.insert(
            "L-tasks".into(),
            vec![
                due("T1", "Renew passport", &two_ago),
                due("T2", "Dentist", &yesterday),
                due("T3", "Bins out", &today),
                done_late,
                task("T5", "Someday", "W/\"T5\""),
            ],
        );
        data.tasks
            .insert("L-groc".into(), vec![due("G1", "Oat milk", &yesterday)]);
    });
    env.synced();

    // The plan, across every list.
    let plan = env.json(&["reschedule", "--overdue", "--to", "tomorrow", "--dry-run"]);
    assert_eq!(plan["dry_run"], true);
    let mut planned: Vec<&str> = plan["targets"]
        .as_array()
        .map(|targets| targets.iter().filter_map(|t| t["title"].as_str()).collect())
        .unwrap_or_default();
    planned.sort_unstable();
    assert_eq!(planned, ["Dentist", "Oat milk", "Renew passport"]);

    // Off a terminal, more than one task needs --yes; nothing is queued.
    let queued = env.outbox().len();
    let refused = env.failure(
        &[
            "reschedule",
            "--overdue",
            "--list",
            "Tasks",
            "--to",
            "tomorrow",
        ],
        2,
    );
    assert!(
        refused["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("--yes")),
        "{refused}"
    );
    assert_eq!(env.outbox().len(), queued);

    let moved = env.json(&[
        "reschedule",
        "--overdue",
        "--list",
        "Tasks",
        "--to",
        "tomorrow",
        "--yes",
    ]);
    assert_eq!(moved["action"], "edit");
    assert_eq!(titles(&moved), ["Renew passport", "Dentist"]);
    let op = moved["op_id"].as_str().expect("op_id").to_owned();
    env.settled();
    assert_eq!(due_of(&env, "Tasks", "Renew passport"), tomorrow);
    assert_eq!(due_of(&env, "Tasks", "Dentist"), tomorrow);
    assert_eq!(due_of(&env, "Groceries", "Oat milk"), yesterday, "--list");
    let commands: Vec<Value> = env
        .outbox()
        .into_iter()
        .filter(|write| write["command_id"] == op.as_str())
        .collect();
    assert_eq!(commands.len(), 2, "one operation per task, one command");

    // Nothing overdue is left in Tasks: a change to nothing.
    let none = env.json(&[
        "reschedule",
        "--overdue",
        "--list",
        "Tasks",
        "--to",
        "tomorrow",
    ]);
    assert_eq!(none["items"], json!([]));

    // The dentist moves again; undo puts the passport back and leaves the
    // dentist where it now is, saying so.
    let dentist = env.local_id(&["tasks", "list"], "T2");
    env.json(&["tasks", "edit", &dentist, "--due", "+5d"]);
    env.settled();
    let undone = env.json(&["undo", &op]);
    assert_eq!(undone["undoes"], op.as_str());
    assert_eq!(titles(&undone), ["Renew passport"]);
    assert_eq!(undone["refused"][0]["title"], "Dentist");
    assert!(
        undone["refused"][0]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("dueDateTime")),
        "{undone}"
    );
    env.settled();
    assert_eq!(due_of(&env, "Tasks", "Renew passport"), two_ago);
    // Put back as the local day's midnight in the user's zone: Graph keeps
    // only the date part of what it's sent, so the UTC instant it gave
    // (23:00 the day before, in summer) would land a day early.
    let last_patch: Value = graph
        .requests("PATCH")
        .await
        .last()
        .map(|request| serde_json::from_slice(&request.body).expect("json"))
        .unwrap_or_default();
    assert_eq!(
        last_patch["dueDateTime"],
        json!({ "dateTime": format!("{two_ago}T00:00:00"), "timeZone": "Europe/London" })
    );
    assert_eq!(due_of(&env, "Tasks", "Dentist"), london_day(&env, 5));
}

#[tokio::test]
async fn an_undo_is_refused_when_every_task_changed_since() {
    let mut env = Env::new();
    let graph = graph_with(&mut env, Vec::new(), Vec::new()).await;
    env.synced();
    let yesterday = london_day(&env, -1);
    graph.edit(|data| {
        data.tasks.insert(
            "L-tasks".into(),
            vec![due("T1", "One", &yesterday), due("T2", "Two", &yesterday)],
        );
    });
    env.synced();
    let moved = env.json(&["reschedule", "--overdue", "--to", "tomorrow", "--yes"]);
    let op = moved["op_id"].as_str().expect("op_id").to_owned();
    let ids = field(&moved, "id").join("\n");
    env.cmd()
        .args([
            "--format",
            "json",
            "reschedule",
            "-",
            "--to",
            "+3d",
            "--yes",
        ])
        .write_stdin(ids)
        .assert()
        .success();
    env.settled();
    let queued = env.outbox().len();
    let refused = env.failure(&["undo", &op], 5);
    assert_eq!(refused["error"]["kind"], "conflict");
    assert_eq!(env.outbox().len(), queued, "nothing queued");
}

#[tokio::test]
async fn a_bulk_edit_takes_ids_from_stdin_and_one_task_fields_are_refused() {
    let mut env = Env::new();
    let _graph = graph_with(
        &mut env,
        vec![
            task("T1", "One", "W/\"T1\""),
            task("T2", "Two", "W/\"T2\""),
            task("T3", "Three", "W/\"T3\""),
        ],
        Vec::new(),
    )
    .await;
    env.synced();
    let ids = format!(
        "{}\n{}\n",
        env.local_id(&["tasks", "list"], "T1"),
        env.local_id(&["tasks", "list"], "T2")
    );

    // stdin can't also answer a question: several need --yes.
    let output = env
        .cmd()
        .args([
            "--format",
            "json",
            "tasks",
            "edit",
            "-",
            "--importance",
            "high",
        ])
        .write_stdin(ids.clone())
        .assert()
        .code(2)
        .get_output()
        .clone();
    assert!(String::from_utf8_lossy(&output.stderr).contains("--yes"));

    let output = env
        .cmd()
        .args([
            "--format",
            "json",
            "tasks",
            "edit",
            "-",
            "--importance",
            "high",
            "--reminder",
            "tomorrow 9am",
            "--yes",
        ])
        .write_stdin(ids.clone())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let edited: Value = serde_json::from_slice(&output).expect("json");
    assert_eq!(titles(&edited), ["One", "Two"]);
    assert_eq!(field(&edited, "importance"), ["high", "high"]);
    env.settled();
    let importance: Vec<String> = env.json(&["tasks", "list"])["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|task| format!("{} {}", task["title"], task["importance"]))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        importance,
        [
            "\"One\" \"high\"",
            "\"Two\" \"high\"",
            "\"Three\" \"normal\""
        ]
    );

    // One title for two tasks makes no sense.
    let output = env
        .cmd()
        .args([
            "--format", "json", "tasks", "edit", "-", "--title", "Same", "--yes",
        ])
        .write_stdin(ids)
        .assert()
        .code(2)
        .get_output()
        .clone();
    assert!(String::from_utf8_lossy(&output.stderr).contains("one task at a time"));
}
