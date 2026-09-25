//! Lists made, renamed and deleted through the real binary and daemon,
//! against a fake Graph (rung 8e): each change shows in the cache at once,
//! goes through the outbox, and `undo` reverses it while nothing has
//! changed since. Microsoft To Do's own lists are refused, and a delete of
//! a list with tasks can't be undone.

mod support;

use serde_json::{Value, json};
use support::Env;
use support::fake_graph::{FakeGraph, list, task};

async fn graph(env: &mut Env) -> FakeGraph {
    let graph = FakeGraph::start(
        env,
        vec![
            list("L-tasks", "Tasks", "defaultList"),
            list("L-flag", "Flagged Emails", "flaggedEmails"),
            list("L-groc", "Groceries", "none"),
        ],
    )
    .await;
    graph.edit(|data| {
        data.tasks.insert("L-tasks".into(), Vec::new());
        data.tasks.insert("L-flag".into(), Vec::new());
        data.tasks
            .insert("L-groc".into(), vec![task("T-milk", "Milk", "W/\"m\"")]);
    });
    graph.accept_catalog().await;
    graph.accept_moves().await;
    env.synced();
    graph
}

fn names(env: &Env) -> Vec<String> {
    env.json(&["lists", "list"])["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|list| list["displayName"].as_str().unwrap_or_default().to_owned())
        .collect()
}

#[tokio::test]
async fn a_list_made_here_shows_at_once_takes_tasks_and_reaches_graph() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;

    let plan = env.json(&["lists", "create", "Garden", "--folder", "Home", "--dry-run"]);
    assert_eq!(plan["dry_run"], true);
    assert_eq!(plan["action"], "create_list");
    assert_eq!(
        plan["lists"][0]["changes"],
        json!({ "displayName": "Garden", "folder": "Home" })
    );
    assert!(
        graph.list_named("Garden").is_none(),
        "a dry run sends nothing"
    );

    let made = env.json(&["lists", "create", "Garden", "--folder", "Home"]);
    assert_eq!(made["action"], "create_list");
    let id = made["items"][0]["id"].as_str().expect("id").to_owned();
    assert_eq!(made["items"][0]["graph_id"], Value::Null);
    assert_eq!(made["items"][0]["folder"], "Home");
    // A task can go in at once: its create waits for the list's.
    let added = env.json(&[
        "tasks",
        "add",
        "Plant bulbs",
        "--no-parse",
        "--list",
        "Garden",
    ]);
    assert_eq!(added["list_ids"], json!([id]));

    env.settled();
    let on_graph = graph.list_named("Garden").expect("created");
    let graph_id = on_graph["id"].as_str().expect("graph id");
    assert_eq!(
        graph.extension(graph_id).expect("folder written")["folder"],
        "Home"
    );
    assert_eq!(
        graph.tasks_in(graph_id).len(),
        1,
        "the task followed the list"
    );
    env.synced();
    let shown = env.json(&["lists", "show", "Garden"]);
    assert_eq!(shown["id"], id, "the list keeps its local ID");
    assert_eq!(shown["graph_id"], graph_id);
    assert_eq!(shown["open_count"], 1);
    assert_eq!(
        names(&env).iter().filter(|name| *name == "Garden").count(),
        1,
        "a sync never makes it twice"
    );
}

#[tokio::test]
async fn a_rename_is_undone_while_the_name_holds_and_built_in_lists_are_refused() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;

    let renamed = env.json(&["lists", "rename", "Groceries", "Shopping"]);
    assert_eq!(renamed["action"], "rename_list");
    assert_eq!(renamed["items"][0]["displayName"], "Shopping");
    env.settled();
    assert!(graph.list_named("Shopping").is_some());
    let undone = env.json(&["undo"]);
    assert_eq!(undone["undoes"], renamed["op_id"]);
    env.settled();
    assert!(graph.list_named("Groceries").is_some());
    assert!(names(&env).contains(&"Groceries".to_owned()));

    for (args, what) in [
        (
            &["lists", "rename", "Tasks", "Inbox"][..],
            "can't be renamed",
        ),
        (
            &["lists", "delete", "Flagged Emails", "--yes"][..],
            "can't be deleted",
        ),
        (&["lists", "create", "Groceries"][..], "already"),
        (&["lists", "rename", "Groceries", "Tasks"][..], "already"),
        (&["lists", "create", "  "][..], "empty"),
    ] {
        let error = env.failure(args, 2);
        let message = error["error"]["message"].as_str().unwrap_or_default();
        assert!(message.contains(what), "{args:?}: {message}");
    }
    // A rename made on the phone since: undo leaves it alone.
    env.json(&["lists", "rename", "Groceries", "Food"]);
    env.settled();
    graph.edit(|data| {
        if let Some(found) = data.lists.iter_mut().find(|list| list["id"] == "L-groc") {
            found["displayName"] = json!("Market");
        }
    });
    env.synced();
    let refused = env.failure(&["undo"], 5);
    assert_eq!(refused["error"]["kind"], "conflict");
}

#[tokio::test]
async fn an_empty_lists_delete_is_undone_and_a_full_ones_is_asked_for_and_final() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    env.json(&["lists", "create", "Scratch", "--folder", "Home"]);
    env.settled();
    let old = graph.list_named("Scratch").expect("made")["id"]
        .as_str()
        .expect("id")
        .to_owned();
    let id = env.local_id(&["lists", "list"], &old);

    // Off a terminal, a delete needs --yes, and says what it would take.
    let error = env.failure(&["lists", "delete", "Scratch"], 2);
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("--yes")
    );
    let plan = env.json(&["lists", "delete", "Scratch", "--dry-run"]);
    assert_eq!(
        plan["lists"][0]["changes"],
        json!({ "deleted": true, "tasks": 0, "undoable": true })
    );

    env.json(&["lists", "delete", "Scratch", "--yes"]);
    assert!(
        !names(&env).contains(&"Scratch".to_owned()),
        "gone from the cache at once"
    );
    env.settled();
    assert!(graph.list_named("Scratch").is_none());

    env.json(&["undo"]);
    env.settled();
    let back = graph.list_named("Scratch").expect("made again");
    let new = back["id"].as_str().expect("id");
    assert_ne!(new, old, "a new Graph ID");
    assert_eq!(graph.extension(new).expect("folder")["folder"], "Home");
    assert_eq!(
        env.local_id(&["lists", "list"], new),
        id,
        "the same local ID"
    );

    // Groceries holds a task: deleting it takes the task, and is final.
    let plan = env.json(&["lists", "delete", "Groceries", "--dry-run"]);
    assert_eq!(plan["lists"][0]["changes"]["tasks"], 1);
    env.json(&["lists", "delete", "Groceries", "--yes"]);
    env.settled();
    assert!(graph.list_named("Groceries").is_none());
    let refused = env.failure(&["undo"], 7);
    assert!(
        refused["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("can't be undone"),
        "{refused}"
    );
}

#[tokio::test]
async fn a_list_with_writes_waiting_isnt_deleted_under_them() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    // The list's create takes a while, and the task's waits for it.
    graph
        .stall(
            "POST",
            r"^/v1\.0/me/todo/lists$",
            Some(ms_todo_fake_graph::catalog::create_list),
            std::time::Duration::from_secs(2),
        )
        .await;
    env.json(&["lists", "create", "Later"]);
    env.json(&["tasks", "add", "Think", "--no-parse", "--list", "Later"]);
    let error = env.failure(&["lists", "delete", "Later", "--yes"], 2);
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("waiting to be sent"),
        "{error}"
    );
    env.settled();
    env.json(&["lists", "delete", "Later", "--yes"]);
    env.settled();
    assert!(graph.list_named("Later").is_none());
}

#[tokio::test]
async fn a_list_deleted_before_its_tasks_have_synced_counts_what_graph_holds() {
    let mut env = Env::new();
    let graph = FakeGraph::start(
        &mut env,
        vec![
            list("L-tasks", "Tasks", "defaultList"),
            list("L-groc", "Groceries", "none"),
        ],
    )
    .await;
    graph.edit(|data| {
        data.tasks.insert("L-tasks".into(), Vec::new());
        data.tasks
            .insert("L-groc".into(), vec![task("T-milk", "Milk", "W/\"m\"")]);
        // The first sync reads the lists at once, their tasks slowly.
        data.tasks_delay = Some(std::time::Duration::from_secs(3));
    });
    graph.accept_catalog().await;
    env.json(&["sync"]);
    let listed = env.json(&["lists", "list"]);
    assert_eq!(listed["items"].as_array().map(Vec::len), Some(2));
    assert!(
        env.json(&["tasks", "list", "--list", "Groceries"])["items"]
            .as_array()
            .is_some_and(Vec::is_empty),
        "the cache has none of its tasks yet"
    );

    let plan = env.json(&["lists", "delete", "Groceries", "--dry-run"]);
    assert_eq!(
        plan["lists"][0]["changes"],
        json!({ "deleted": true, "tasks": 1, "undoable": false }),
        "Graph was asked, not the empty cache"
    );
    env.json(&["lists", "delete", "Groceries", "--yes"]);
    env.settled();
    let refused = env.failure(&["undo"], 7);
    assert!(
        refused["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("can't be undone"),
        "{refused}"
    );
}
