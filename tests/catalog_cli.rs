//! Outlook categories and open extensions through the real binary and
//! daemon, against a fake Graph (rung 8e, D-058): each write goes straight
//! to Graph with a dry run, needs `--yes` off a terminal to delete, and is
//! undone by `undo` while nothing changed it since.

mod support;

use serde_json::{Value, json};
use support::Env;
use support::fake_graph::{FakeGraph, list, task};

use ms_todo_fake_graph::catalog::category;

async fn graph(env: &mut Env) -> FakeGraph {
    let graph = FakeGraph::start(
        env,
        vec![
            list("L-tasks", "Tasks", "defaultList"),
            list("L-home", "Home", "none"),
        ],
    )
    .await;
    graph.edit(|data| {
        data.tasks.insert("L-tasks".into(), Vec::new());
        data.tasks.insert(
            "L-home".into(),
            vec![task("T-fix", "Fix the gate", "W/\"f\"")],
        );
        data.categories = vec![category("c-bills", "Bills", "preset3")];
        data.extensions.insert(
            "T-fix".into(),
            json!({ "extensionName": "com.planetaryescape.mstodo", "myDay": "2026-09-25" }),
        );
    });
    graph.accept_catalog().await;
    env.synced();
    graph
}

fn message(error: &Value) -> &str {
    error["error"]["message"].as_str().unwrap_or_default()
}

#[tokio::test]
async fn categories_are_listed_made_recoloured_and_deleted_and_each_undone() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;

    let listed = env.json(&["categories", "list"]);
    assert_eq!(listed["items"][0]["displayName"], "Bills");
    assert!(
        listed.get("sync").is_none(),
        "read from Graph, not the cache"
    );

    let plan = env.json(&[
        "categories",
        "create",
        "Errands",
        "--color",
        "Preset4",
        "--dry-run",
    ]);
    assert_eq!(
        plan["changes"]["body"],
        json!({ "displayName": "Errands", "color": "preset4" })
    );
    assert!(graph.category("Errands").is_none());

    let made = env.json(&["categories", "create", "Errands", "--color", "preset4"]);
    assert_eq!(made["action"], "category_create");
    assert_eq!(made["items"][0]["color"], "preset4");
    assert!(graph.category("errands").is_some());
    // Names are unique ignoring case (S7).
    let error = env.failure(&["categories", "create", "ERRANDS"], 2);
    assert!(message(&error).contains("exists already"), "{error}");
    let error = env.failure(&["categories", "create", "Gym", "--color", "teal"], 2);
    assert!(message(&error).contains("preset0"), "{error}");

    env.json(&["categories", "recolor", "errands", "--color", "preset7"]);
    assert_eq!(
        graph.category("Errands").expect("there")["color"],
        "preset7"
    );
    env.json(&["undo"]);
    assert_eq!(
        graph.category("Errands").expect("there")["color"],
        "preset4"
    );

    // Off a terminal, a delete needs --yes.
    let error = env.failure(&["categories", "delete", "Bills"], 2);
    assert!(message(&error).contains("--yes"), "{error}");
    let deleted = env.json(&["categories", "delete", "Bills", "--yes"]);
    assert_eq!(deleted["action"], "category_delete");
    assert!(graph.category("Bills").is_none());
    let undone = env.json(&["undo"]);
    assert_eq!(undone["undoes"], deleted["op_id"]);
    let back = graph.category("Bills").expect("made again");
    assert_eq!(back["color"], "preset3", "with its colour");

    // A create is undone by a delete, unless it was recoloured since.
    let made = env.json(&["categories", "create", "Gym"]);
    graph.edit(|data| {
        if let Some(gym) = data
            .categories
            .iter_mut()
            .find(|found| found["displayName"] == "Gym")
        {
            gym["color"] = json!("preset9");
        }
    });
    let op = made["op_id"].as_str().expect("op_id");
    let refused = env.failure(&["undo", op], 5);
    assert!(message(&refused).contains("recoloured since"), "{refused}");
    assert!(graph.category("Gym").is_some());
    let missing = env.failure(&["categories", "recolor", "Nope", "--color", "none"], 3);
    assert_eq!(missing["error"]["kind"], "not_found");
}

#[tokio::test]
async fn open_extensions_are_read_set_and_deleted_on_lists_and_tasks() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;

    let set = env.json(&[
        "extensions",
        "set",
        "list",
        "Home",
        "com.example.garden",
        "--json",
        r#"{"beds": 3, "soil": "clay"}"#,
    ]);
    assert_eq!(set["action"], "extension_set");
    let stored = graph
        .named_extension("L-home", "com.example.garden")
        .expect("made");
    assert_eq!(stored["beds"], 3);
    // Graph can't list extensions, so `list` is ms-todo's own: Home has
    // none.
    let listed = env.json(&["extensions", "list", "list", "Home"]);
    assert_eq!(listed["items"], json!([]));
    let got = env.json(&["extensions", "get", "list", "Home", "com.example.garden"]);
    assert_eq!(got["soil"], "clay");

    // A set replaces the whole document; undo puts the old one back.
    env.json(&[
        "extensions",
        "set",
        "list",
        "Home",
        "com.example.garden",
        "--json",
        r#"{"beds": 4}"#,
    ]);
    assert_eq!(
        graph
            .named_extension("L-home", "com.example.garden")
            .expect("there")["soil"],
        Value::Null
    );
    env.json(&["undo"]);
    let restored = graph
        .named_extension("L-home", "com.example.garden")
        .expect("there");
    assert_eq!(
        (restored["beds"].clone(), restored["soil"].clone()),
        (json!(3), json!("clay"))
    );

    // On a task, named by its title in a list; Graph can't list its,
    // so `list` gives ms-todo's own.
    env.json(&[
        "extensions",
        "set",
        "task",
        "Fix the gate",
        "--list",
        "Home",
        "com.example.hw",
        "--json",
        r#"{"part": "hinge"}"#,
    ]);
    assert_eq!(
        graph
            .named_extension("T-fix", "com.example.hw")
            .expect("made")["part"],
        "hinge"
    );
    let ours = env.json(&[
        "extensions",
        "list",
        "task",
        "Fix the gate",
        "--list",
        "Home",
    ]);
    assert_eq!(
        ours["items"][0]["extensionName"],
        "com.planetaryescape.mstodo"
    );

    let error = env.failure(
        &[
            "extensions",
            "delete",
            "task",
            "Fix the gate",
            "--list",
            "Home",
            "com.example.hw",
        ],
        2,
    );
    assert!(message(&error).contains("--yes"), "{error}");
    env.json(&[
        "extensions",
        "delete",
        "task",
        "Fix the gate",
        "--list",
        "Home",
        "com.example.hw",
        "--yes",
    ]);
    assert!(graph.named_extension("T-fix", "com.example.hw").is_none());
    env.json(&["undo"]);
    assert_eq!(
        graph
            .named_extension("T-fix", "com.example.hw")
            .expect("back")["part"],
        "hinge"
    );

    // ms-todo's own is read-only here; bad JSON and a missing one are refused.
    for (args, code, what) in [
        (
            &[
                "extensions",
                "set",
                "list",
                "Home",
                "com.planetaryescape.mstodo",
                "--json",
                "{\"a\":1}",
            ][..],
            2,
            "ms-todo's own",
        ),
        (
            &["extensions", "set", "list", "Home", "x.y", "--json", "[1]"][..],
            2,
            "object",
        ),
        (
            &["extensions", "set", "list", "Home", "x.y", "--json", "{}"][..],
            2,
            "at least one",
        ),
        (
            &["extensions", "get", "list", "Home", "x.none"][..],
            3,
            "no extension",
        ),
    ] {
        let error = env.failure(args, code);
        assert!(message(&error).contains(what), "{args:?}: {error}");
    }
    assert!(
        graph.extension("T-fix").expect("ours")["myDay"] == "2026-09-25",
        "ms-todo's own is untouched"
    );
}
