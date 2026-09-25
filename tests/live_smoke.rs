//! The live smoke test (rung 8e, docs/blueprint/03-graph-provider.md#testing):
//! the whole command surface against a real Microsoft account, in a list
//! and a category it makes for itself and deletes at the end, whatever
//! happens. Off by default: it needs the `live-smoke` feature, `--ignored`
//! and an instance that's signed in:
//!
//! ```sh
//! MS_TODO_LIVE_INSTANCE=livetest MS_TODO_MY_DAY_TODAY=2000-01-01 \
//!   cargo test --features live-smoke --test live_smoke -- --ignored --nocapture
//! ```
//!
//! It writes only to what it makes: lists and a category named
//! `ms-todo-spike-8e-smoke-<time>`, and open extensions on those lists
//! and their tasks. `MS_TODO_MY_DAY_TODAY` keeps the My Day rollover
//! away from every other task (a debug build reads it).

#![cfg(feature = "live-smoke")]

use std::process::Output;
use std::time::{Duration, Instant};

use assert_cmd::Command;
use serde_json::{Value, json};

struct Live {
    instance: String,
    prefix: String,
    /// Lists and categories made, by ID, for the clean-up.
    lists: Vec<String>,
    categories: Vec<String>,
}

impl Live {
    fn new() -> Self {
        let instance = std::env::var("MS_TODO_LIVE_INSTANCE")
            .expect("set MS_TODO_LIVE_INSTANCE to a signed-in instance, such as livetest");
        assert!(
            !instance.is_empty() && instance != "default",
            "run it against a separate instance, never the default one"
        );
        let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S");
        Self {
            instance,
            prefix: format!("ms-todo-spike-8e-smoke-{stamp}"),
            lists: Vec::new(),
            categories: Vec::new(),
        }
    }

    fn output(&self, args: &[&str]) -> Output {
        let mut command = Command::cargo_bin("ms-todo").expect("ms-todo binary");
        command
            .args(["--instance", &self.instance, "--format", "json"])
            .args(args);
        if std::env::var_os("MS_TODO_MY_DAY_TODAY").is_none() {
            command.env("MS_TODO_MY_DAY_TODAY", "2000-01-01");
        }
        command.output().expect("run ms-todo")
    }

    /// Run `args`, expect success, and return its JSON.
    fn json(&self, args: &[&str]) -> Value {
        let output = self.output(args);
        assert!(
            output.status.success(),
            "{args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap_or(Value::Null)
    }

    /// Run `args` and expect it to exit `code`.
    fn fails(&self, args: &[&str], code: i32) -> Value {
        let output = self.output(args);
        assert_eq!(
            output.status.code(),
            Some(code),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stderr).unwrap_or(Value::Null)
    }

    /// Wait until the outbox has sent everything, and fail on anything
    /// it couldn't.
    fn settle(&self) {
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            let items = self.json(&["outbox", "list"])["items"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let busy = items
                .iter()
                .any(|op| op["state"] == "pending" || op["state"] == "inflight");
            if !busy {
                let stuck: Vec<&Value> = items
                    .iter()
                    .filter(|op| op["state"] == "unknown" || op["state"] == "failed")
                    .filter(|op| op["created_at"].as_i64() > Some(self.started()))
                    .collect();
                assert!(stuck.is_empty(), "writes that didn't land: {stuck:?}");
                return;
            }
            assert!(Instant::now() < deadline, "the outbox never settled");
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    /// Unix seconds this run began, from the prefix's stamp.
    fn started(&self) -> i64 {
        let stamp = self.prefix.rsplit('-').next().unwrap_or_default();
        chrono::NaiveDateTime::parse_from_str(stamp, "%Y%m%d%H%M%S")
            .map(|at| at.and_utc().timestamp())
            .unwrap_or(0)
    }

    fn graph(&self, path: &str) -> Value {
        self.json(&["raw", "GET", path])
    }

    fn list_graph_id(&self, local: &str) -> String {
        let shown = self.json(&["lists", "show", local]);
        shown["graph_id"].as_str().expect("a Graph ID").to_owned()
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        for list in self.lists.clone() {
            let _ = self.output(&["lists", "delete", &list, "--yes"]);
        }
        for category in self.categories.clone() {
            let _ = self.output(&["categories", "delete", &category, "--yes"]);
        }
        let _ = self.output(&["sync", "--wait"]);
    }
}

#[test]
#[ignore = "writes to a real Microsoft account; run by hand with --features live-smoke"]
fn the_whole_surface_against_a_real_account() {
    let mut live = Live::new();
    live.json(&["sync", "--wait"]);
    let name = live.prefix.clone();
    let other = format!("{name}-b");

    // Lists: create (in a folder), show, rename and undo, a second list.
    let made = live.json(&["lists", "create", &name, "--folder", &name]);
    let list = made["items"][0]["id"].as_str().expect("id").to_owned();
    live.lists.push(list.clone());
    let made = live.json(&["lists", "create", &other]);
    let second = made["items"][0]["id"].as_str().expect("id").to_owned();
    live.lists.push(second.clone());
    live.settle();
    let graph_list = live.list_graph_id(&list);
    assert_eq!(
        live.graph(&format!("/me/todo/lists/{graph_list}"))["displayName"],
        name
    );
    live.json(&["lists", "rename", &list, &format!("{name}-renamed")]);
    live.settle();
    live.json(&["undo"]);
    live.settle();
    assert_eq!(
        live.graph(&format!("/me/todo/lists/{graph_list}"))["displayName"],
        name
    );
    live.fails(&["lists", "delete", &list], 2);

    // Categories: create, recolour, undo, list.
    let category = live.json(&["categories", "create", &name, "--color", "preset3"]);
    live.categories
        .push(category["items"][0]["id"].as_str().expect("id").to_owned());
    live.json(&["categories", "recolor", &name, "--color", "preset4"]);
    live.json(&["undo"]);
    let listed = live.json(&["categories", "list"]);
    let ours = listed["items"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|found| found["displayName"] == name.as_str())
        .expect("the category");
    assert_eq!(ours["color"], "preset3");

    // A task with every field, then edits of each.
    let added = live.json(&[
        "tasks",
        "add",
        "Smoke task",
        "--no-parse",
        "--list",
        &list,
        "--due",
        "tomorrow",
        "--category",
        &name,
        "--recur",
        "every mon",
        "--reminder",
        "tomorrow 9am",
        "--importance",
        "high",
        "--body",
        "Made by the live smoke test",
    ]);
    let task = added["items"][0]["id"].as_str().expect("id").to_owned();
    live.settle();
    let shown = live.json(&["tasks", "show", &task]);
    let graph_task = shown["graph_id"].as_str().expect("graph id").to_owned();
    let on_graph = live.graph(&format!("/me/todo/lists/{graph_list}/tasks/{graph_task}"));
    assert_eq!(on_graph["recurrence"]["pattern"]["type"], "weekly");
    assert_eq!(on_graph["categories"], json!([name]));
    // A repeating task keeps no start date of its own (D-058).
    live.fails(&["tasks", "edit", &task, "--start", "tomorrow"], 2);

    for edit in [
        &["--clear-recur"][..],
        &["--start", "tomorrow"][..],
        &["--clear-start"][..],
        &["--start", "in 3 days"][..],
        // With a start date, the start moves to the first occurrence too.
        &["--recur", "every 2 weeks on tue, thu"][..],
        &["--clear-categories"][..],
        &["--category", &name][..],
        &["--title", "Smoke task, edited"][..],
        &["--assignee", "Smoke"][..],
        &["--clear-assignee"][..],
    ] {
        let mut args = vec!["tasks", "edit", task.as_str()];
        args.extend_from_slice(edit);
        live.json(&args);
        live.settle();
    }
    let on_graph = live.graph(&format!("/me/todo/lists/{graph_list}/tasks/{graph_task}"));
    assert_eq!(on_graph["title"], "Smoke task, edited");
    assert_eq!(on_graph["recurrence"]["pattern"]["interval"], 2);
    let day = |key: &str| {
        on_graph[key]["dateTime"]
            .as_str()
            .map(|at| at[..10].to_owned())
    };
    assert_eq!(day("startDateTime"), day("dueDateTime"), "{on_graph}");
    live.json(&["undo"]);
    live.settle();

    // Reads and filters.
    let found = live.json(&["tasks", "list", "--category", &name]);
    assert_eq!(found["items"].as_array().map(Vec::len), Some(1));
    live.json(&[
        "tasks", "list", "--list", &list, "--status", "open", "--sort", "due",
    ]);
    live.json(&["tasks", "list", "--due", "any", "--limit", "3"]);
    live.json(&["--fresh", "search", "Smoke", "--list", &list]);
    live.json(&["lists", "show", &list]);
    live.json(&["folders", "list"]);
    live.json(&["waiting"]);
    live.json(&["done", "--list", &list]);
    live.json(&[
        "reschedule",
        "--due-before",
        "+30d",
        "--list",
        &list,
        "--to",
        "tomorrow",
        "--dry-run",
    ]);

    // Children: steps, a link, a small attachment.
    live.json(&["steps", "add", &task, "One", "Two"]);
    live.settle();
    live.json(&["steps", "check", &task, "1"]);
    live.json(&[
        "links",
        "add",
        &task,
        "https://example.com/smoke",
        "--name",
        "Smoke",
    ]);
    let file = tempfile::NamedTempFile::new().expect("temp file");
    std::fs::write(file.path(), b"ms-todo live smoke test\n").expect("write");
    live.json(&[
        "attachments",
        "add",
        &task,
        file.path().to_str().expect("path"),
    ]);
    live.settle();
    let out = tempfile::tempdir().expect("temp dir");
    live.json(&[
        "attachments",
        "download",
        &task,
        "--out",
        out.path().to_str().expect("dir"),
    ]);
    live.json(&["steps", "delete", &task, "2", "--yes"]);
    live.json(&["links", "delete", &task, "--yes"]);
    live.json(&["attachments", "delete", &task, "1", "--yes", "--no-undo"]);
    live.settle();

    // Extensions on the list and the task.
    let extension = "com.planetaryescape.spike8e";
    live.json(&[
        "extensions",
        "set",
        "list",
        &list,
        extension,
        "--json",
        r#"{"smoke": true}"#,
    ]);
    assert_eq!(
        live.json(&["extensions", "get", "list", &list, extension])["smoke"],
        true
    );
    live.json(&["extensions", "list", "list", &list]);
    live.json(&[
        "extensions",
        "set",
        "task",
        &task,
        extension,
        "--json",
        r#"{"smoke": 1}"#,
    ]);
    live.json(&["extensions", "get", "task", &task, extension]);
    live.json(&["extensions", "delete", "task", &task, extension, "--yes"]);
    live.json(&["extensions", "delete", "list", &list, extension, "--yes"]);
    live.fails(&["extensions", "get", "list", &list, extension], 3);

    // My Day, complete, reopen, move there and back, delete and undo.
    live.json(&["myday", "add", &task]);
    live.settle();
    live.json(&["myday", "remove", &task]);
    live.settle();
    live.json(&["tasks", "move", &task, "--to", &second]);
    live.settle();
    live.json(&["undo"]);
    live.settle();
    // A recurring task's completion makes a copy undo would ask about.
    // Completing turns the reminder off, and reopening leaves it off,
    // which a move then trips on (docs/issues/005): so it comes last.
    live.json(&["tasks", "edit", &task, "--clear-recur"]);
    live.settle();
    live.json(&["tasks", "complete", &task]);
    live.settle();
    live.json(&["undo"]);
    live.settle();
    live.json(&["tasks", "delete", &task, "--yes"]);
    live.settle();
    live.json(&["undo"]);
    live.settle();

    // Categories: delete and undo, then the clean-up deletes it for good.
    live.json(&["categories", "delete", &name, "--yes"]);
    let back = live.json(&["undo"]);
    live.categories = vec![back["items"][0]["id"].as_str().expect("id").to_owned()];

    // An empty list's delete is undone; a full one's is final.
    live.json(&["lists", "delete", &second, "--yes"]);
    live.settle();
    live.json(&["undo"]);
    live.settle();
    live.json(&["lists", "delete", &list, "--yes"]);
    live.settle();
    live.fails(&["undo"], 7);
    live.fails(&["raw", "GET", &format!("/me/todo/lists/{graph_list}")], 3);
    live.lists.retain(|id| *id != list);
}
