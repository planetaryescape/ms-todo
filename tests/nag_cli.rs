//! Nag reminders through the real binary and daemon, against a fake Graph
//! (rung 9b, D-063): `nag` in our extension, set, cleared and undone like
//! other extension edits; refused without a reminder; `--nagging`; quick
//! add's `+nag15m`; and the daemon's nagger, whose notifications a
//! debug-build hook writes to a file instead of the screen.

mod support;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use support::Env;
use support::fake_graph::{FakeGraph, list, task};

fn ours(fields: Value) -> Value {
    let mut extension = json!({
        "extensionName": "com.planetaryescape.mstodo",
        "id": "microsoft.graph.openTypeExtension.com.planetaryescape.mstodo"
    });
    if let (Some(extension), Some(fields)) = (extension.as_object_mut(), fields.as_object()) {
        extension.extend(fields.clone());
    }
    extension
}

/// A task whose reminder has passed: due to nag.
fn reminded(id: &str, title: &str) -> Value {
    let mut task = task(id, title, &format!("W/\"{id}\""));
    task["isReminderOn"] = json!(true);
    task["reminderDateTime"] = json!({
        "dateTime": "2026-09-01T09:00:00.0000000",
        "timeZone": "Europe/London"
    });
    task
}

async fn graph_with(env: &mut Env, tasks: Vec<Value>, extensions: Vec<(&str, Value)>) -> FakeGraph {
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
        for (id, extension) in extensions {
            data.extensions.insert(id.to_owned(), extension);
        }
    });
    graph.accept_task_patches().await;
    // Creates, which `tasks add` sends.
    graph.accept_moves().await;
    graph
}

/// No quiet hours, so the tests pass at any time of day.
fn no_quiet_hours(env: &Env) {
    let dir = env.home.path().join("config").join("ms-todo");
    std::fs::create_dir_all(&dir).expect("config dir");
    std::fs::write(dir.join("config.toml"), "[nag]\nquiet_hours = \"\"\n").expect("config");
}

/// Restart the daemon with notifications going to a file and a nag's
/// minute lasting 100 ms (debug-build hooks). Returns the file.
fn start_nagging(env: &Env) -> PathBuf {
    let file = env.home.path().join("notifications.txt");
    env.json(&["daemon", "stop"]);
    env.cmd()
        .env("MS_TODO_NAG_NOTIFY_FILE", &file)
        .env("MS_TODO_NAG_MINUTE_MS", "100")
        .args(["--format", "json", "daemon", "start"])
        .assert()
        .success();
    file
}

fn notifications(file: &Path) -> Vec<String> {
    std::fs::read_to_string(file)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Wait until `file` holds at least `count` notifications.
fn wait_for(file: &Path, count: usize) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let shown = notifications(file);
        if shown.len() >= count || Instant::now() > deadline {
            return shown;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[tokio::test]
async fn a_nag_is_set_listed_cleared_and_undone_and_needs_a_reminder() {
    let mut env = Env::new();
    let (with_reminder, without) = (reminded("T1", "Call mum"), task("T2", "Tidy", "W/\"2\""));
    let graph = graph_with(&mut env, vec![with_reminder, without], Vec::new()).await;
    env.synced();
    let id = env.local_id(&["tasks", "list"], "T1");
    let bare = env.local_id(&["tasks", "list"], "T2");

    let refused = env.failure(&["tasks", "nag", &id, &bare, "--every", "15m"], 2);
    assert_eq!(refused["error"]["kind"], "invalid_input");
    let message = refused["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("\"Tidy\" has no reminder"), "{message}");
    assert!(env.outbox().is_empty(), "nothing queued for either task");
    // Too often is a usage error, before the daemon.
    env.cmd()
        .args(["tasks", "nag", &id, "--every", "2m"])
        .assert()
        .code(2);

    let plan = env.json(&["tasks", "nag", &id, "--every", "1h", "--dry-run"]);
    assert_eq!(plan["changes"]["nag"], 60);

    let set = env.json(&["tasks", "nag", &id, "--every", "15m"]);
    assert_eq!(set["action"], "edit");
    assert_eq!(set["items"][0]["extensions"][0]["nag"], 15, "at once");
    env.settled();
    assert_eq!(graph.extension("T1").expect("extension")["nag"], 15);

    let nagging = env.json(&["tasks", "list", "--nagging"]);
    let ids: Vec<&str> = nagging["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter_map(|task| task["id"].as_str())
        .collect();
    assert_eq!(ids, [id.as_str()]);
    assert_eq!(nagging["items"][0]["list"], "Tasks", "every list's shape");

    let off = env.json(&["tasks", "nag", &id, "--off"]);
    env.settled();
    // Nothing else was in it, so the extension itself went.
    let left = graph.extension("T1");
    assert!(
        left.as_ref()
            .is_none_or(|ours| ours.get("nag").is_none_or(Value::is_null)),
        "{left:?}"
    );
    assert_eq!(
        env.json(&["tasks", "list", "--nagging"])["items"],
        json!([])
    );

    env.json(&["undo", off["op_id"].as_str().expect("op_id")]);
    env.settled();
    assert_eq!(graph.extension("T1").expect("extension")["nag"], 15, "back");

    // `tasks edit` takes it too, with a reminder in the same edit.
    let edited = env.json(&[
        "tasks",
        "edit",
        &bare,
        "--reminder",
        "tomorrow 9am",
        "--nag",
        "30m",
    ]);
    assert_eq!(edited["items"][0]["extensions"][0]["nag"], 30);
    let cleared = env.json(&["tasks", "edit", &bare, "--clear-nag"]);
    assert!(cleared["items"][0]["extensions"][0]["nag"].is_null());
}

#[tokio::test]
async fn a_new_task_nags_with_a_reminder_from_a_flag_or_the_text() {
    let mut env = Env::new();
    let graph = graph_with(&mut env, Vec::new(), Vec::new()).await;
    env.synced();

    let refused = env.failure(&["tasks", "add", "Call mum", "--nag", "15m"], 2);
    let message = refused["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("needs --reminder"), "{message}");

    let parsed = env.json(&["tasks", "parse", "Call mum !9am +nag1h"]);
    assert_eq!(parsed["nag"], 60);
    assert_eq!(parsed["title"], "Call mum");

    let added = env.json(&["tasks", "add", "Call mum !9am +nag1h"]);
    assert_eq!(added["items"][0]["extensions"][0]["nag"], 60);
    let flagged = env.json(&[
        "tasks",
        "add",
        "Pay rent",
        "--reminder",
        "fri 9am",
        "--nag",
        "5m",
    ]);
    assert_eq!(flagged["items"][0]["extensions"][0]["nag"], 5);
    env.settled();
    let nags: Vec<Value> = graph
        .tasks_in("L-tasks")
        .iter()
        .filter_map(|task| graph.extension(task["id"].as_str()?))
        .map(|extension| extension["nag"].clone())
        .collect();
    assert_eq!(nags.len(), 2, "{nags:?}");
    assert!(
        nags.contains(&json!(60)) && nags.contains(&json!(5)),
        "{nags:?}"
    );
}

#[tokio::test]
async fn the_daemon_nags_until_the_task_is_completed() {
    let mut env = Env::new();
    let nagging = reminded("T1", "Call mum\u{1b}[31m");
    let quiet = reminded("T2", "Not nagging");
    graph_with(
        &mut env,
        vec![nagging, quiet],
        vec![("T1", ours(json!({ "nag": 5 })))],
    )
    .await;
    no_quiet_hours(&env);
    let file = start_nagging(&env);
    env.synced();
    let id = env.local_id(&["tasks", "list"], "T1");

    // Due at once (its reminder was yesterday), then every 5 × 100 ms.
    let shown = wait_for(&file, 2);
    assert!(shown.len() >= 2, "{shown:?}");
    assert!(
        shown.iter().all(|line| line == "Call mum[31m\tTasks"),
        "the title made one safe line, and the list: {shown:?}"
    );
    let logs = env.json(&["daemon", "logs"]);
    let log = std::fs::read_to_string(logs["path"].as_str().expect("log path")).unwrap_or_default();
    assert!(log.contains(&format!("nag: notified {id}")), "{log}");

    env.json(&["tasks", "complete", &id]);
    let after = notifications(&file).len();
    std::thread::sleep(Duration::from_millis(1500));
    // One may have been on its way as the completion landed.
    assert!(notifications(&file).len() <= after + 1, "stopped");
    let settled = notifications(&file).len();
    std::thread::sleep(Duration::from_millis(1000));
    assert_eq!(notifications(&file).len(), settled);

    let doctor = env.json(&["doctor"]);
    assert_eq!(doctor["nag"]["active"], true);
    assert_eq!(doctor["nag"]["notifier"], "file");
    assert_eq!(doctor["nag"]["count"], 0);
    assert!(doctor["nag"]["quiet_hours"].is_null());
}

#[tokio::test]
async fn doctor_sends_a_test_notification_and_reads_the_quiet_hours() {
    let mut env = Env::new();
    graph_with(&mut env, Vec::new(), Vec::new()).await;
    let file = start_nagging(&env);
    env.synced();
    let doctor = env.json(&["doctor", "--notify-test"]);
    assert_eq!(doctor["nag"]["quiet_hours"], "22:00-07:00", "the default");
    let shown = notifications(&file);
    assert_eq!(shown.len(), 1, "{shown:?}");
    assert!(shown[0].starts_with("ms-todo\t"), "{shown:?}");
}

#[tokio::test]
async fn a_deferred_or_someday_task_waits_until_it_shows_again() {
    let mut env = Env::new();
    let tasks = vec![
        reminded("T1", "Deferred"),
        reminded("T2", "Someday"),
        reminded("T3", "Visible"),
    ];
    let extensions = vec![
        ("T1", ours(json!({ "nag": 5, "deferUntil": "2099-01-01" }))),
        ("T2", ours(json!({ "nag": 5, "someday": true }))),
        ("T3", ours(json!({ "nag": 5 }))),
    ];
    graph_with(&mut env, tasks, extensions).await;
    no_quiet_hours(&env);
    let file = start_nagging(&env);
    env.synced();

    // The visible one nags every 500 ms; give the others as long.
    let shown = wait_for(&file, 3);
    assert!(shown.len() >= 3, "{shown:?}");
    assert!(
        shown.iter().all(|line| line == "Visible\tTasks"),
        "only the visible task: {shown:?}"
    );

    // Back in sight, it nags.
    let deferred = env.local_id(&["tasks", "list", "--deferred", "include"], "T1");
    env.json(&["tasks", "edit", &deferred, "--clear-defer"]);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !notifications(&file)
        .iter()
        .any(|line| line.starts_with("Deferred\t"))
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        notifications(&file)
            .iter()
            .any(|line| line.starts_with("Deferred\t")),
        "nags once it shows"
    );
}

#[tokio::test]
async fn a_nag_turned_off_and_on_nags_again_at_once() {
    let mut env = Env::new();
    let nagging = reminded("T1", "Call mum");
    graph_with(
        &mut env,
        vec![nagging],
        vec![("T1", ours(json!({ "nag": 5 })))],
    )
    .await;
    no_quiet_hours(&env);
    let file = env.home.path().join("notifications.txt");
    env.json(&["daemon", "stop"]);
    // A minute of 2 s: the interval is 10 s, and a tick every second.
    env.cmd()
        .env("MS_TODO_NAG_NOTIFY_FILE", &file)
        .env("MS_TODO_NAG_MINUTE_MS", "2000")
        .args(["--format", "json", "daemon", "start"])
        .assert()
        .success();
    env.synced();
    let id = env.local_id(&["tasks", "list"], "T1");
    assert_eq!(wait_for(&file, 1).len(), 1, "the first, at once");

    env.json(&["tasks", "nag", &id, "--off"]);
    env.json(&["tasks", "nag", &id, "--every", "5m"]);
    // Well inside the 10 s interval the old time would have waited out.
    let started = Instant::now();
    let shown = wait_for(&file, 2);
    assert_eq!(shown.len(), 2, "{shown:?}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn without_a_notifier_doctor_complains_only_once_a_task_nags() {
    let mut env = Env::new();
    graph_with(&mut env, vec![reminded("T1", "Call mum")], Vec::new()).await;
    env.json(&["daemon", "stop"]);
    // As on Linux with no D-Bus session bus.
    env.cmd()
        .env("MS_TODO_NAG_NO_NOTIFIER", "1")
        .args(["--format", "json", "daemon", "start"])
        .assert()
        .success();
    env.synced();
    let doctor = env.json(&["doctor"]);
    assert!(doctor["nag"]["notifier"].is_null());
    assert!(doctor["nag"]["problem"].is_null(), "{doctor}");
    assert_eq!(doctor["problems"], json!([]));

    let id = env.local_id(&["tasks", "list"], "T1");
    env.json(&["tasks", "nag", &id, "--every", "15m"]);
    let doctor = env.json(&["doctor"]);
    let problem = doctor["nag"]["problem"].as_str().unwrap_or_default();
    assert!(problem.contains("nothing nags here"), "{doctor}");
    let failed = env.failure(&["doctor", "--notify-test"], 7);
    assert_eq!(failed["error"]["kind"], "unsupported");
}
