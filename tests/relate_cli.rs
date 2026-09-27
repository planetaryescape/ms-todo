//! Related tasks (D-067) through the real binary and daemon, against a fake
//! Graph: `tasks relate` keeps each task's Graph ID in the other's
//! extension (`related`), through the whole-document write that keeps
//! every other field; `tasks show` names them; `unrelate` and `undo` take
//! the link away both ways.

mod support;

use serde_json::{Value, json};
use support::Env;
use support::fake_graph::{FakeGraph, list, task};

/// Graph with "Tasks" holding T1, which carries an extension from
/// elsewhere, and "Groceries" holding G1.
async fn graph(env: &mut Env) -> FakeGraph {
    let graph = FakeGraph::start(
        env,
        vec![
            list("L-tasks", "Tasks", "defaultList"),
            list("L-groc", "Groceries", "none"),
        ],
    )
    .await;
    graph.edit(|data| {
        data.tasks
            .insert("L-tasks".into(), vec![task("T1", "Plan party", "W/\"t1\"")]);
        data.tasks
            .insert("L-groc".into(), vec![task("G1", "Buy cake", "W/\"g1\"")]);
        data.extensions.insert(
            "T1".into(),
            json!({
                "extensionName": "com.planetaryescape.mstodo",
                "id": "microsoft.graph.openTypeExtension.com.planetaryescape.mstodo",
                "assignee": "Sam"
            }),
        );
    });
    graph.accept_task_patches().await;
    graph.accept_moves().await;
    graph.accept_catalog().await;
    env.synced();
    graph
}

fn related(graph: &FakeGraph, task: &str) -> Value {
    graph
        .extension(task)
        .map_or(Value::Null, |extension| extension["related"].clone())
}

#[tokio::test]
async fn two_tasks_link_both_ways_show_by_title_and_unlink() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    let party = env.local_id(&["tasks", "list", "--list", "Tasks"], "T1");
    let cake = env.local_id(&["tasks", "list", "--list", "Groceries"], "G1");

    let plan = env.json(&["tasks", "relate", &party, &cake, "--dry-run"]);
    assert_eq!(plan["action"], "relate");
    assert_eq!(plan["changes"][&party]["related"], json!(["G1"]));
    assert_eq!(plan["changes"][&cake]["related"], json!(["T1"]));
    assert!(graph.writes().await.is_empty(), "a dry run writes nothing");

    let linked = env.json(&["tasks", "relate", &party, &cake]);
    assert_eq!(linked["action"], "relate");
    env.settled();
    assert_eq!(related(&graph, "T1"), json!(["G1"]));
    assert_eq!(related(&graph, "G1"), json!(["T1"]));
    assert_eq!(
        graph.extension("T1").expect("extension")["assignee"],
        "Sam",
        "the rest of the document is kept"
    );

    let shown = env.json(&["tasks", "show", &party]);
    assert_eq!(shown["related"][0]["title"], "Buy cake");
    assert_eq!(shown["related"][0]["id"], cake.as_str());
    assert_eq!(shown["related"][0]["graph_id"], "G1");
    let table = env
        .cmd()
        .args(["--format", "table", "tasks", "show", &cake])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let table = String::from_utf8(table).expect("utf8");
    assert!(table.contains("Plan party"), "{table}");

    // Undo takes the link away both ways, keeping the rest.
    env.json(&["undo", linked["op_id"].as_str().expect("op_id")]);
    env.settled();
    assert_eq!(related(&graph, "T1"), Value::Null);
    assert_eq!(related(&graph, "G1"), Value::Null);
    assert_eq!(graph.extension("T1").expect("extension")["assignee"], "Sam");

    // Moved, a task has a new Graph ID; a link to its old one still finds
    // it, and the copy keeps its own link.
    env.json(&["tasks", "relate", &party, &cake]);
    env.settled();
    env.json(&["tasks", "move", &cake, "--to", "Tasks"]);
    env.settled();
    let shown = env.json(&["tasks", "show", &party]);
    assert_eq!(shown["related"][0]["id"], cake.as_str());
    assert_eq!(shown["related"][0]["title"], "Buy cake");
    let copy = env.json(&["tasks", "show", &cake]);
    let copy_id = copy["graph_id"].as_str().expect("graph id").to_owned();
    assert_ne!(copy_id, "G1");
    assert_eq!(copy["related"][0]["title"], "Plan party");

    // Linked again, the old ID is brought up to date; then it's no change.
    env.json(&["tasks", "relate", &cake, &party]);
    env.settled();
    assert_eq!(related(&graph, "T1"), json!([copy_id]));
    let again = env.json(&["tasks", "relate", &cake, &party]);
    assert_eq!(again["items"], json!([]));

    // Named by title, both must be in --list.
    env.failure(
        &[
            "tasks",
            "unrelate",
            "Plan party",
            "Buy cake",
            "--list",
            "Groceries",
        ],
        3,
    );
    env.json(&[
        "tasks",
        "unrelate",
        "Plan party",
        "Buy cake",
        "--list",
        "Tasks",
    ]);
    env.settled();
    assert_eq!(related(&graph, "T1"), Value::Null);
    assert_eq!(related(&graph, &copy_id), Value::Null);
}

#[tokio::test]
async fn undoing_one_link_leaves_a_later_one_and_both_sides_agree() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    graph.edit(|data| {
        data.tasks.get_mut("L-groc").expect("groceries").push(task(
            "G2",
            "Buy candles",
            "W/\"g2\"",
        ));
    });
    env.synced();
    let party = env.local_id(&["tasks", "list", "--list", "Tasks"], "T1");
    let cake = env.local_id(&["tasks", "list", "--list", "Groceries"], "G1");
    let candles = env.local_id(&["tasks", "list", "--list", "Groceries"], "G2");
    let first = env.json(&["tasks", "relate", &party, &cake]);
    env.settled();
    env.json(&["tasks", "relate", &party, &candles]);
    env.settled();
    assert_eq!(related(&graph, "T1"), json!(["G1", "G2"]));

    let undone = env.json(&["undo", first["op_id"].as_str().expect("op_id")]);
    assert!(undone.get("refused").is_none(), "{undone}");
    env.settled();
    assert_eq!(related(&graph, "T1"), json!(["G2"]), "the later link stays");
    assert_eq!(related(&graph, "G1"), Value::Null, "gone on both sides");
    assert_eq!(related(&graph, "G2"), json!(["T1"]));
}

#[tokio::test]
async fn a_task_is_not_linked_to_itself_or_to_one_graph_has_not_got() {
    let mut env = Env::new();
    let _graph = graph(&mut env).await;
    let party = env.local_id(&["tasks", "list", "--list", "Tasks"], "T1");
    env.failure(&["tasks", "relate", &party, &party], 2);
    env.failure(&["tasks", "relate", &party, "no-such-task"], 3);
}
