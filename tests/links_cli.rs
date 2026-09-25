//! `tasks links` and `tasks open` through the real binary and daemon,
//! against a fake Graph. Opening with one link would start a browser, so
//! that path is covered by `link_commands`' unit tests with a fake opener;
//! here `open` only refuses.

mod support;

use serde_json::{Value, json};
use support::Env;
use support::fake_graph::{FakeGraph, list, task};

/// "Tasks" with one task holding a linked resource and two URLs in its
/// notes (one of them the linked resource's again), and one with a
/// `javascript:` resource.
async fn synced_env() -> (Env, FakeGraph) {
    let mut env = Env::new();
    let graph = FakeGraph::start(&mut env, vec![list("L-tasks", "Tasks", "defaultList")]).await;
    let mut linked = task("T1", "Read the thread", "W/\"T1\"");
    linked["body"] = json!({
        "content": "From https://mail.example.com/m/1, see [the docs](https://docs.example.com/a).",
        "contentType": "text"
    });
    linked["linkedResources"] = json!([
        { "webUrl": "https://mail.example.com/m/1", "displayName": "The email", "applicationName": "Outlook" }
    ]);
    let mut evil = task("T2", "Evil", "W/\"T2\"");
    evil["linkedResources"] = json!([{ "webUrl": "javascript:alert(1)", "applicationName": "x" }]);
    graph.edit(|data| {
        data.tasks.insert(
            "L-tasks".into(),
            vec![linked, evil, task("T3", "Plain", "W/\"T3\"")],
        );
    });
    env.synced();
    (env, graph)
}

fn run(env: &Env, args: &[&str]) -> (Option<i32>, String, String) {
    let output = env.cmd().args(args).output().expect("run");
    (
        output.status.code(),
        String::from_utf8(output.stdout).expect("utf8"),
        String::from_utf8(output.stderr).expect("utf8"),
    )
}

#[tokio::test]
async fn links_lists_linked_resources_first_then_the_notes_each_once() {
    let (env, _graph) = synced_env().await;
    let result: Value = env.json(&["tasks", "links", "Read the thread", "--list", "Tasks"]);
    assert_eq!(result["sync"]["state"], "ready");
    let items = result["items"].as_array().expect("items");
    let summary: Vec<(u64, &str, &str, &str, bool)> = items
        .iter()
        .map(|item| {
            (
                item["index"].as_u64().expect("index"),
                item["url"].as_str().expect("url"),
                item["text"].as_str().expect("text"),
                item["source"].as_str().expect("source"),
                item["openable"].as_bool().expect("openable"),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            (
                1,
                "https://mail.example.com/m/1",
                "The email",
                "linked_resource",
                true
            ),
            (2, "https://docs.example.com/a", "the docs", "notes", true),
        ]
    );

    let (code, csv, _) = run(
        &env,
        &[
            "--format",
            "csv",
            "tasks",
            "links",
            "Read the thread",
            "--list",
            "Tasks",
        ],
    );
    assert_eq!(code, Some(0));
    assert_eq!(
        csv,
        "url,text,source\nhttps://mail.example.com/m/1,The email,linked_resource\nhttps://docs.example.com/a,the docs,notes\n"
    );
    let (_, ids, _) = run(
        &env,
        &[
            "--format",
            "ids",
            "tasks",
            "links",
            "Read the thread",
            "--list",
            "Tasks",
        ],
    );
    assert_eq!(
        ids,
        "https://mail.example.com/m/1\nhttps://docs.example.com/a\n"
    );
    let (_, jsonl, _) = run(
        &env,
        &[
            "--format",
            "jsonl",
            "tasks",
            "links",
            "Read the thread",
            "--list",
            "Tasks",
        ],
    );
    assert_eq!(jsonl.lines().count(), 2);
    let (_, table, _) = run(
        &env,
        &[
            "--format",
            "table",
            "tasks",
            "links",
            "Read the thread",
            "--list",
            "Tasks",
        ],
    );
    assert!(
        table.contains("The email") && table.contains("SOURCE"),
        "{table}"
    );
}

#[tokio::test]
async fn open_never_blocks_on_a_choice_or_opens_another_scheme() {
    let (env, _graph) = synced_env().await;
    // Several links and no --index: they're listed, exit 2, nothing opened.
    let (code, _, stderr) = run(
        &env,
        &[
            "--format",
            "table",
            "tasks",
            "open",
            "Read the thread",
            "--list",
            "Tasks",
        ],
    );
    assert_eq!(code, Some(2), "{stderr}");
    assert!(
        stderr.contains("--index") && stderr.contains("2  the docs  https://docs.example.com/a"),
        "{stderr}"
    );
    let (code, _, stderr) = run(
        &env,
        &[
            "--format", "table", "tasks", "open", "Evil", "--list", "Tasks",
        ],
    );
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("only http, https and mailto"), "{stderr}");
    let (code, _, stderr) = run(
        &env,
        &[
            "--format", "table", "tasks", "open", "Plain", "--list", "Tasks",
        ],
    );
    assert_eq!(code, Some(3), "{stderr}");
    assert!(stderr.contains("no links"), "{stderr}");
}
