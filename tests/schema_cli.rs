//! `ms-todo schema [CMD]`: each command's input and output schemas at
//! schema_version 2, snapshotted so a change is a deliberate update
//! (docs/blueprint/07-cli.md#output-contract), and checked against what
//! the commands really print.

mod support;

use assert_cmd::Command;
use serde_json::{Value, json};
use support::Env;
use support::fake_graph::{FakeGraph, list, task};
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

const COMMANDS: &[&str] = &[
    "auth login",
    "auth status",
    "auth logout",
    "auth bearer",
    "lists list",
    "lists move",
    "lists order",
    "folders list",
    "folders rename",
    "folders delete",
    "folders order",
    "tasks list",
    "tasks add",
    "tasks parse",
    "tasks suggest-list",
    "tasks complete",
    "tasks reopen",
    "tasks edit",
    "tasks move",
    "tasks delete",
    "tasks links",
    "tasks open",
    "steps list",
    "steps add",
    "steps edit",
    "steps check",
    "steps uncheck",
    "steps delete",
    "links list",
    "links add",
    "links edit",
    "links delete",
    "attachments list",
    "attachments add",
    "attachments download",
    "attachments delete",
    "waiting",
    "myday list",
    "myday add",
    "myday remove",
    "myday suggest",
    "myday rollover",
    "search",
    "done",
    "reschedule",
    "outbox list",
    "outbox retry",
    "outbox discard",
    "undo",
    "sync",
    "doctor",
    "schema",
    "raw",
    "daemon start",
    "daemon stop",
    "daemon status",
];

fn schema(args: &[&str]) -> Value {
    let output = Command::cargo_bin("ms-todo")
        .expect("ms-todo binary")
        .arg("schema")
        .args(args)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    serde_json::from_slice(&output).expect("json")
}

#[test]
fn every_commands_schema_matches_its_snapshot() {
    for command in COMMANDS {
        let words: Vec<&str> = command.split(' ').collect();
        let described = schema(&words);
        assert_eq!(described["schema_version"], 2);
        assert_eq!(described["command"], *command);
        let name = format!("schema_{}", command.replace(' ', "_"));
        let shapes = json!({ "input": described["input"], "output": described["output"] });
        let pretty = serde_json::to_string_pretty(&shapes).expect("json");
        insta::assert_snapshot!(name, pretty);
    }
    // The same for every command.
    let error = serde_json::to_string_pretty(&schema(&[])["error"]).expect("json");
    insta::assert_snapshot!("schema_error", error);
}

#[test]
fn without_a_command_it_describes_every_command() {
    let all = schema(&[]);
    let mut listed: Vec<&str> = all["commands"]
        .as_object()
        .expect("commands")
        .keys()
        .map(String::as_str)
        .collect();
    listed.sort_unstable();
    let mut expected = COMMANDS.to_vec();
    expected.sort_unstable();
    assert_eq!(listed, expected);
    assert_eq!(
        all["commands"]["tasks list"]["output"],
        schema(&["tasks", "list"])["output"]
    );
}

#[test]
fn an_unknown_command_exits_2() {
    Command::cargo_bin("ms-todo")
        .expect("ms-todo binary")
        .args(["schema", "tasks", "fly"])
        .assert()
        .code(2);
}

/// Every key `schema` requires is in `value`, recursively through objects,
/// arrays and `oneOf` (the first alternative that fits).
fn assert_conforms(schema: &Value, value: &Value, at: &str) {
    if let Some(options) = schema["oneOf"].as_array() {
        let fits = options.iter().any(|option| {
            option["required"].as_array().is_none_or(|keys| {
                keys.iter()
                    .all(|key| value.get(key.as_str().unwrap_or("")).is_some())
            })
        });
        assert!(fits, "{at}: {value} fits none of {schema}");
        return;
    }
    if let Some(required) = schema["required"].as_array() {
        for key in required.iter().filter_map(Value::as_str) {
            assert!(value.get(key).is_some(), "{at}: missing {key:?} in {value}");
        }
    }
    if let (Some(properties), Some(object)) = (schema["properties"].as_object(), value.as_object())
    {
        for (key, property) in properties {
            if let Some(inner) = object.get(key).filter(|inner| !inner.is_null()) {
                assert_conforms(property, inner, &format!("{at}.{key}"));
            }
        }
    }
    if let (Some(items), Some(array)) = (schema.get("items"), value.as_array()) {
        for (index, item) in array.iter().enumerate() {
            assert_conforms(items, item, &format!("{at}[{index}]"));
        }
    }
}

#[tokio::test]
async fn real_output_has_every_field_its_schema_requires() {
    let mut env = Env::new();
    let graph = FakeGraph::start(&mut env, vec![list("L-tasks", "Tasks", "defaultList")]).await;
    graph.edit(|data| {
        let mut paid = task("T3", "Pay rent", "W/\"e3\"");
        paid["status"] = json!("completed");
        paid["completedDateTime"] =
            json!({ "dateTime": "2026-09-24T00:00:00.0000000", "timeZone": "UTC" });
        data.tasks.insert(
            "L-tasks".into(),
            vec![task("T1", "Buy milk", "W/\"e1\""), paid],
        );
    });
    Mock::given(method("POST"))
        .and(path("/v1.0/me/todo/lists/L-tasks/tasks"))
        .respond_with(ResponseTemplate::new(201).set_body_json(task("T2", "Eggs", "W/\"n\"")))
        .mount(&graph.server)
        .await;

    let cases: &[(&str, &[&str])] = &[
        ("sync", &["sync", "--wait"]),
        ("lists list", &["lists", "list"]),
        ("tasks list", &["tasks", "list"]),
        ("tasks list", &["tasks", "list", "--search", "milk"]),
        ("search", &["search", "milk"]),
        ("done", &["done", "--since", "2026-09-01"]),
        (
            "reschedule",
            &["reschedule", "--overdue", "--to", "tomorrow", "--dry-run"],
        ),
        ("tasks add", &["tasks", "add", "Eggs"]),
        ("tasks add", &["tasks", "add", "Eggs", "--dry-run"]),
        (
            "tasks parse",
            &["tasks", "parse", "Eggs every mon #Tasks p1 9am"],
        ),
        ("outbox list", &["outbox", "list"]),
        ("undo", &["undo"]),
        (
            "lists move",
            &["lists", "move", "Tasks", "--folder", "Areas"],
        ),
        (
            "lists move",
            &["lists", "move", "Tasks", "--no-folder", "--dry-run"],
        ),
        ("folders list", &["folders", "list"]),
        ("folders rename", &["folders", "rename", "Areas", "Work"]),
        ("undo", &["undo"]),
        ("doctor", &["doctor"]),
        ("daemon status", &["daemon", "status"]),
    ];
    for (command, args) in cases {
        let words: Vec<&str> = command.split(' ').collect();
        let output = schema(&words)["output"].clone();
        assert_conforms(&output, &env.json(args), command);
    }
    assert_conforms(
        &schema(&["tasks", "list"])["error"],
        &json!({ "error": { "kind": "not_found", "message": "x" } }),
        "error",
    );
}
