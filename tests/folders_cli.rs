//! Folders through the real binary and daemon, against a fake Graph
//! (docs/blueprint/05-custom-features.md#folders-list-groups): each change
//! is a write of our extension on each list it moves, merged over what
//! Graph holds, queued in the outbox and undoable; `lists list` and
//! `folders list` read them back from the cache.

mod support;

use serde_json::{Value, json};
use support::Env;
use support::fake_graph::{FakeGraph, list};

/// Graph with the default list and four more, signed in and synced.
async fn graph(env: &mut Env) -> FakeGraph {
    let graph = FakeGraph::start(
        env,
        vec![
            list("L-tasks", "Tasks", "defaultList"),
            list("L-fin", "Finances", "none"),
            list("L-health", "Health", "none"),
            list("L-launch", "Launch", "none"),
            list("L-groc", "Groceries", "none"),
        ],
    )
    .await;
    graph.edit(|data| {
        for id in ["L-tasks", "L-fin", "L-health", "L-launch", "L-groc"] {
            data.tasks.insert(id.into(), Vec::new());
        }
    });
    env.synced();
    graph
}

/// `lists list` as `(name, folder)` pairs, in the order printed.
fn lists(env: &Env) -> Vec<(String, Option<String>)> {
    env.json(&["lists", "list"])["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|list| {
            (
                list["displayName"].as_str().unwrap_or_default().to_owned(),
                list["folder"].as_str().map(str::to_owned),
            )
        })
        .collect()
}

fn pair(name: &str, folder: Option<&str>) -> (String, Option<String>) {
    (name.to_owned(), folder.map(str::to_owned))
}

fn folder_names(env: &Env) -> Vec<String> {
    env.json(&["folders", "list"])["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|folder| folder["name"].as_str().unwrap_or_default().to_owned())
        .collect()
}

#[tokio::test]
async fn a_bulk_move_writes_each_lists_extension_without_losing_its_other_fields() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    // Something another ms-todo wrote, which the merge must keep.
    graph.edit(|data| {
        data.extensions.insert(
            "L-fin".into(),
            json!({ "extensionName": "com.planetaryescape.mstodo", "keep": "me" }),
        );
    });

    let moved = env.json(&["lists", "move", "Finances", "Health", "--folder", "Areas"]);
    assert_eq!(moved["action"], "move_list");
    let items = moved["items"].as_array().expect("items");
    assert_eq!(items.len(), 2);
    assert!(items.iter().all(|item| item["folder"] == "Areas"));
    assert!(items.iter().all(|item| item["sync_state"] == "pending"));
    // Offline-safe: the cache shows it before Graph has it.
    assert_eq!(lists(&env)[0], pair("Finances", Some("Areas")));

    env.settled();
    let fin = graph.extension("L-fin").expect("written");
    assert_eq!(fin["folder"], "Areas");
    assert_eq!(
        fin["keep"], "me",
        "the whole document was merged, not replaced"
    );
    assert_eq!(
        graph.extension("L-health").expect("created")["folder"],
        "Areas"
    );

    env.synced();
    assert_eq!(
        lists(&env),
        [
            pair("Finances", Some("Areas")),
            pair("Health", Some("Areas")),
            pair("Tasks", None),
            pair("Launch", None),
            pair("Groceries", None),
        ]
    );
    let folders = env.json(&["folders", "list"]);
    assert_eq!(folders["items"][0]["name"], "Areas");
    assert_eq!(folders["items"][0]["list_count"], 2);
    assert_eq!(
        folders["items"][0]["lists"].as_array().map(Vec::len),
        Some(2)
    );

    // A folder that exists matches ignoring case, and keeps its spelling.
    env.json(&["lists", "move", "Groceries", "--folder", "areas"]);
    assert_eq!(folder_names(&env), ["Areas"]);
    // Groceries' extension holds nothing else, so it's deleted: Graph
    // refuses to PATCH an empty document.
    env.json(&["lists", "move", "Groceries", "--no-folder"]);
    env.settled();
    assert_eq!(graph.extension("L-groc"), None);
    // Finances keeps its other field.
    env.json(&["lists", "move", "Finances", "--no-folder"]);
    env.settled();
    assert!(env.outbox().iter().all(|op| op["state"] == "done"));
    let fin = graph.extension("L-fin").expect("kept");
    assert_eq!(fin["keep"], "me");
    assert!(fin.get("folder").is_none());
}

#[tokio::test]
async fn a_dry_run_plans_and_writes_nothing() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    let plan = env.json(&[
        "lists",
        "move",
        "Finances",
        "--folder",
        "Areas",
        "--dry-run",
    ]);
    assert_eq!(plan["dry_run"], true);
    assert_eq!(plan["action"], "move_list");
    assert_eq!(plan["lists"][0]["name"], "Finances");
    assert_eq!(plan["lists"][0]["changes"]["folder"], "Areas");
    assert!(env.outbox().is_empty());
    assert!(graph.writes().await.is_empty());
    assert_eq!(lists(&env)[1], pair("Finances", None));
}

#[tokio::test]
async fn rename_moves_every_list_and_delete_keeps_the_lists() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    env.json(&["lists", "move", "Finances", "Health", "--folder", "Areas"]);

    let renamed = env.json(&["folders", "rename", "Areas", "Responsibilities"]);
    assert_eq!(renamed["action"], "rename_folder");
    assert_eq!(renamed["items"].as_array().map(Vec::len), Some(2));
    assert_eq!(folder_names(&env), ["Responsibilities"]);
    env.json(&["lists", "move", "Launch", "--folder", "Projects"]);
    let taken = env.failure(&["folders", "rename", "Responsibilities", "projects"], 2);
    assert_eq!(taken["error"]["kind"], "invalid_input");

    let deleted = env.json(&["folders", "delete", "Responsibilities", "--yes"]);
    assert_eq!(deleted["action"], "delete_folder");
    assert_eq!(folder_names(&env), ["Projects"]);
    let names: Vec<String> = lists(&env).into_iter().map(|(name, _)| name).collect();
    assert_eq!(names.len(), 5, "no list was deleted: {names:?}");
    env.settled();
    assert_eq!(
        graph.extension("L-fin"),
        None,
        "its only fields were the folder's"
    );

    // Without a terminal, delete needs --yes.
    let refused = env.failure(&["folders", "delete", "Projects"], 2);
    assert_eq!(refused["error"]["kind"], "invalid_input");
    let missing = env.failure(&["folders", "delete", "Nope", "--yes"], 3);
    assert_eq!(missing["error"]["kind"], "not_found");
}

#[tokio::test]
async fn lists_and_folders_take_the_order_given() {
    let mut env = Env::new();
    let _graph = graph(&mut env).await;
    env.json(&["lists", "move", "Finances", "Health", "--folder", "Areas"]);
    env.json(&["lists", "move", "Launch", "--folder", "Projects"]);
    assert_eq!(folder_names(&env), ["Areas", "Projects"]);

    env.json(&["lists", "order", "Health", "--before", "Finances"]);
    env.json(&["folders", "order", "Projects", "--before", "Areas"]);
    assert_eq!(folder_names(&env), ["Projects", "Areas"]);
    assert_eq!(
        lists(&env),
        [
            pair("Launch", Some("Projects")),
            pair("Health", Some("Areas")),
            pair("Finances", Some("Areas")),
            pair("Tasks", None),
            pair("Groceries", None),
        ]
    );
    let across = env.failure(&["lists", "order", "Launch", "--after", "Health"], 2);
    assert_eq!(across["error"]["kind"], "invalid_input");
}

#[tokio::test]
async fn undo_puts_a_move_back() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    env.json(&["lists", "move", "Finances", "--folder", "Areas"]);
    let moved = env.json(&["lists", "move", "Finances", "--folder", "Someday"]);
    let undone = env.json(&["undo", moved["op_id"].as_str().expect("op_id")]);
    assert_eq!(undone["action"], "undo");
    assert_eq!(undone["items"][0]["folder"], "Areas");
    env.settled();
    assert_eq!(
        graph.extension("L-fin").expect("written")["folder"],
        "Areas"
    );
    let ops = env.outbox();
    assert!(ops.iter().all(|op| op["state"] == "done"), "{ops:?}");
    assert_eq!(
        ops[0]["title"], "Finances",
        "a list's write is titled by its name"
    );
}

#[tokio::test]
async fn a_folder_set_on_another_machine_arrives_with_the_next_sync() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    graph.edit(|data| {
        data.extensions.insert(
            "L-groc".into(),
            json!({ "extensionName": "com.planetaryescape.mstodo", "folder": "Areas" }),
        );
        let groceries = data
            .lists
            .iter_mut()
            .find(|list| list["id"] == "L-groc")
            .expect("list");
        groceries["@odata.etag"] = Value::String("W/\"L-groc-2\"".into());
    });
    env.synced();
    assert_eq!(lists(&env)[0], pair("Groceries", Some("Areas")));
}
