//! Assignment through the real binary and daemon, against a fake Graph
//! (docs/blueprint/05-custom-features.md#assignment, D-057): `assignee` in
//! our extension, paired with `waitingOnOthers`, `assigneeStatusSet` when
//! ms-todo set that status, the filters, undo, and another machine's
//! change arriving by sync.

mod support;

use serde_json::{Value, json};
use support::Env;
use support::fake_graph::{FakeGraph, list, task};

fn with(mut task: Value, extra: Value) -> Value {
    if let (Some(task), Some(extra)) = (task.as_object_mut(), extra.as_object()) {
        task.extend(extra.clone());
    }
    task
}

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

/// Graph with "Tasks" and "Home", taking task writes as Graph does.
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
    graph.accept_moves().await;
    graph
}

/// My Day's day in these tests: the daemon runs on it, so its rollover
/// leaves a task in that day's My Day alone.
const TODAY: &str = "2026-09-20";

/// Restart the daemon with My Day's day fixed at [`TODAY`] (a debug-build
/// hook).
fn start_on_today(env: &Env) {
    env.json(&["daemon", "stop"]);
    env.cmd()
        .env("MS_TODO_MY_DAY_TODAY", TODAY)
        .args(["--format", "json", "daemon", "start"])
        .assert()
        .success();
}

fn status(graph: &FakeGraph, id: &str) -> Value {
    graph.task("L-tasks", id).expect("task in Graph")["status"].clone()
}

fn titles(items: &Value) -> Vec<String> {
    items
        .as_array()
        .expect("items")
        .iter()
        .map(|item| item["title"].as_str().unwrap_or_default().to_owned())
        .collect()
}

/// A change on the phone or another ms-todo: `change` to task `id`, then
/// a new etag, as Graph gives any write.
fn elsewhere(graph: &FakeGraph, id: &str, change: impl FnOnce(&mut Value)) {
    graph.edit(|data| {
        let task = data
            .tasks
            .get_mut("L-tasks")
            .and_then(|tasks| tasks.iter_mut().find(|task| task["id"] == id))
            .expect("task");
        change(task);
        task["@odata.etag"] = json!(format!("W/\"{id}-elsewhere\""));
    });
}

#[tokio::test]
async fn assigning_makes_a_task_waiting_and_clearing_makes_it_not_started_again() {
    let mut env = Env::new();
    let theirs = ours(json!({ "myDay": TODAY, "opId": "op-old", "keep": "me" }));
    let graph = graph_with(
        &mut env,
        vec![task("T1", "Get the quote", "W/\"1\"")],
        vec![("T1", theirs)],
    )
    .await;
    start_on_today(&env);
    env.synced();
    let id = env.local_id(&["tasks", "list"], "T1");

    let assigned = env.json(&["tasks", "edit", &id, "--assignee", " Sam "]);
    assert_eq!(assigned["action"], "edit");
    let item = &assigned["items"][0];
    assert_eq!(
        item["extensions"][0]["assignee"], "Sam",
        "trimmed, and at once"
    );
    assert_eq!(item["status"], "waitingOnOthers");
    env.settled();
    let extension = graph.extension("T1").expect("extension");
    assert_eq!(extension["assignee"], "Sam");
    assert_eq!(extension["assigneeStatusSet"], true);
    assert_eq!(extension["myDay"], TODAY, "the merge kept My Day");
    assert_eq!(extension["opId"], "op-old", "and the create's opId");
    assert_eq!(extension["keep"], "me", "and a field it doesn't know");
    assert_eq!(status(&graph, "T1"), "waitingOnOthers");

    // The same name again changes nothing.
    let again = env.json(&["tasks", "edit", &id, "--assignee", "Sam"]);
    assert_eq!(again["items"], json!([]));

    let cleared = env.json(&["tasks", "edit", &id, "--clear-assignee"]);
    assert_eq!(cleared["items"][0]["status"], "notStarted");
    env.settled();
    let extension = graph.extension("T1").expect("extension");
    assert!(extension.get("assignee").is_none(), "{extension}");
    assert!(extension.get("assigneeStatusSet").is_none(), "{extension}");
    assert_eq!(extension["myDay"], TODAY);
    assert_eq!(extension["opId"], "op-old");
    assert_eq!(status(&graph, "T1"), "notStarted");
}

#[tokio::test]
async fn a_status_the_user_chose_is_never_overwritten() {
    let mut env = Env::new();
    let graph = graph_with(
        &mut env,
        vec![
            task("T1", "Get the quote", "W/\"1\""),
            with(
                task("T2", "Chase the invoice", "W/\"1\""),
                json!({ "status": "waitingOnOthers" }),
            ),
        ],
        Vec::new(),
    )
    .await;
    env.synced();
    let one = env.local_id(&["tasks", "list"], "T1");
    let two = env.local_id(&["tasks", "list"], "T2");

    // Ms-todo made T1 waiting; the phone then moves it on.
    env.json(&["tasks", "edit", &one, "--assignee", "Sam"]);
    env.settled();
    elsewhere(&graph, "T1", |task| task["status"] = json!("inProgress"));
    env.synced();
    env.json(&["tasks", "edit", &one, "--clear-assignee"]);
    env.settled();
    assert_eq!(
        status(&graph, "T1"),
        "inProgress",
        "the phone's status stays"
    );

    // T2 was waiting by the user's hand: no flag, and clearing keeps it.
    env.json(&["tasks", "edit", &two, "--assignee", "Ada"]);
    env.settled();
    let extension = graph.extension("T2").expect("extension");
    assert_eq!(extension["assignee"], "Ada");
    assert!(extension.get("assigneeStatusSet").is_none(), "{extension}");
    env.json(&["tasks", "edit", &two, "--clear-assignee"]);
    env.settled();
    assert_eq!(status(&graph, "T2"), "waitingOnOthers");

    // --keep-status leaves it alone either way.
    env.json(&["tasks", "edit", &one, "--assignee", "Kim", "--keep-status"]);
    env.settled();
    assert_eq!(status(&graph, "T1"), "inProgress");
    assert!(
        graph
            .extension("T1")
            .expect("extension")
            .get("assigneeStatusSet")
            .is_none()
    );
}

/// The phone makes T1 in progress right after the extension write reads
/// the task back: ms-todo's status edit is skipped.
fn phone_starts_t1(data: &mut support::fake_graph::Data) {
    if let Some(task) = data
        .tasks
        .get_mut("L-tasks")
        .and_then(|tasks| tasks.iter_mut().find(|task| task["id"] == "T1"))
    {
        task["status"] = json!("inProgress");
    }
}

#[tokio::test]
async fn a_status_changed_while_assigning_is_kept_and_never_marked_as_ours() {
    let mut env = Env::new();
    let graph = graph_with(
        &mut env,
        vec![task("T1", "Get the quote", "W/\"1\"")],
        Vec::new(),
    )
    .await;
    graph
        .edit_after_gets(
            r"^/v1\.0/me/todo/lists/L-tasks/tasks/T1$",
            2,
            phone_starts_t1,
        )
        .await;
    env.synced();
    let id = env.local_id(&["tasks", "list"], "T1");
    env.json(&["tasks", "edit", &id, "--assignee", "Sam"]);
    env.settled();
    assert_eq!(status(&graph, "T1"), "inProgress");
    let extension = graph.extension("T1").expect("extension");
    assert_eq!(extension["assignee"], "Sam");
    assert!(extension.get("assigneeStatusSet").is_none(), "{extension}");
    let skipped = env
        .outbox()
        .into_iter()
        .filter(|op| {
            op["note"]
                .as_str()
                .is_some_and(|note| note.starts_with("skipped:"))
        })
        .count();
    assert_eq!(skipped, 2, "the status edit and the flag");
}

#[tokio::test]
async fn adding_with_an_assignee_sends_it_in_the_create() {
    let mut env = Env::new();
    let graph = graph_with(&mut env, Vec::new(), Vec::new()).await;
    env.synced();
    let dry = env.json(&[
        "tasks",
        "add",
        "Get the quote",
        "--assignee",
        "Sam",
        "--dry-run",
    ]);
    assert_eq!(dry["dry_run"], true);
    assert_eq!(dry["changes"]["status"], "waitingOnOthers");
    assert_eq!(dry["changes"]["extensions"][0]["assignee"], "Sam");
    assert!(
        graph.tasks_in("L-tasks").is_empty(),
        "a dry run sends nothing"
    );

    env.json(&["tasks", "add", "Get the quote", "--assignee", "Sam"]);
    env.json(&[
        "tasks",
        "add",
        "--no-parse",
        "Read the draft",
        "--assignee",
        "Ada",
        "--keep-status",
    ]);
    env.settled();
    let created = graph.tasks_in("L-tasks");
    let find = |title: &str| {
        created
            .iter()
            .find(|task| task["title"] == title)
            .cloned()
            .expect("created")
    };
    let quote = find("Get the quote");
    assert_eq!(quote["status"], "waitingOnOthers");
    let extension = graph
        .extension(quote["id"].as_str().expect("id"))
        .expect("ours");
    assert_eq!(extension["assignee"], "Sam");
    assert_eq!(extension["assigneeStatusSet"], true);
    assert!(extension.get("opId").is_some(), "the create's opId kept");
    let draft = find("Read the draft");
    assert_eq!(draft["status"], "notStarted");
    assert_eq!(
        graph
            .extension(draft["id"].as_str().expect("id"))
            .expect("ours")["assignee"],
        "Ada"
    );

    // A name that can't be one is refused before anything is queued.
    let refused = env.failure(&["tasks", "add", "x", "--assignee", "  "], 2);
    assert_eq!(refused["error"]["kind"], "invalid_input");
}

#[tokio::test]
async fn lists_filter_by_person_whatever_the_case_and_waiting_groups_them() {
    let mut env = Env::new();
    let graph = graph_with(
        &mut env,
        vec![
            task("T1", "Get the quote", "W/\"1\""),
            task("T2", "Chase the invoice", "W/\"1\""),
            task("T3", "Water plants", "W/\"1\""),
            with(
                task("T4", "Old favour", "W/\"1\""),
                json!({ "status": "completed" }),
            ),
        ],
        vec![
            ("T1", ours(json!({ "assignee": "Sam" }))),
            ("T2", ours(json!({ "assignee": "ada@example.com" }))),
            ("T4", ours(json!({ "assignee": "sam" }))),
        ],
    )
    .await;
    graph.edit(|data| {
        let home = vec![task("H1", "Fix the gate", "W/\"1\"")];
        data.tasks.insert("L-home".into(), home);
        data.extensions
            .insert("H1".into(), ours(json!({ "assignee": "SAM" })));
    });
    env.synced();

    // Every list's open tasks for Sam, however the name was typed.
    let sam = env.json(&["tasks", "list", "--assignee", "sam"]);
    assert_eq!(titles(&sam["items"]), ["Get the quote", "Fix the gate"]);
    assert_eq!(sam["items"][1]["list"], "Home");
    // Anyone, grouped by person.
    let anyone = env.json(&["tasks", "list", "--assignee", "*"]);
    assert_eq!(
        titles(&anyone["items"]),
        ["Chase the invoice", "Get the quote", "Fix the gate"]
    );
    // The same tasks: compared by title, since a background sync pass may
    // refresh other fields between the two reads.
    assert_eq!(
        titles(&env.json(&["waiting"])["items"]),
        titles(&anyone["items"])
    );
    assert_eq!(
        titles(&env.json(&["waiting", "Ada@Example.com"])["items"]),
        ["Chase the invoice"]
    );
    // In one list, completed ones too, as `tasks list` has them.
    let in_tasks = env.json(&["tasks", "list", "--list", "Tasks", "--assignee", "Sam"]);
    assert_eq!(titles(&in_tasks["items"]), ["Get the quote", "Old favour"]);
    // Nobody by that name.
    assert_eq!(
        env.json(&["tasks", "list", "--assignee", "Kim"])["items"],
        json!([])
    );

    let table = env
        .cmd()
        .args(["--format", "table", "waiting"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let table = String::from_utf8(table).expect("utf-8");
    assert!(table.contains("WAITING ON"), "{table}");
    assert!(table.contains("ada@example.com"), "{table}");
    let csv = env
        .cmd()
        .args(["--format", "csv", "tasks", "list", "--assignee", "Sam"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let csv = String::from_utf8(csv).expect("utf-8");
    assert!(
        csv.starts_with("id,title,assignee,list,status,due,sync_state\n"),
        "{csv}"
    );
    assert!(csv.contains("Fix the gate,SAM,Home,notStarted"), "{csv}");

    let empty = env.failure(&["tasks", "list", "--assignee", " "], 2);
    assert_eq!(empty["error"]["kind"], "invalid_input");
}

#[tokio::test]
async fn a_dry_run_and_several_tasks_at_once() {
    let mut env = Env::new();
    let graph = graph_with(
        &mut env,
        vec![
            task("T1", "Get the quote", "W/\"1\""),
            task("T2", "Chase the invoice", "W/\"1\""),
        ],
        Vec::new(),
    )
    .await;
    env.synced();
    let one = env.local_id(&["tasks", "list"], "T1");
    let two = env.local_id(&["tasks", "list"], "T2");
    let plan = env.json(&[
        "tasks",
        "edit",
        &one,
        &two,
        "--assignee",
        "Sam",
        "--dry-run",
    ]);
    assert_eq!(plan["dry_run"], true);
    assert_eq!(plan["targets"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        plan["changes"],
        json!({ "assignee": "Sam", "status": "waitingOnOthers" })
    );
    assert!(env.outbox().is_empty(), "a dry run queues nothing");

    let changed = env.json(&["tasks", "edit", &one, &two, "--assignee", "Sam", "--yes"]);
    assert_eq!(changed["items"].as_array().map(Vec::len), Some(2));
    env.settled();
    for id in ["T1", "T2"] {
        assert_eq!(graph.extension(id).expect("ours")["assignee"], "Sam");
        assert_eq!(status(&graph, id), "waitingOnOthers");
    }

    // A title and an assignee together, on one task.
    env.json(&[
        "tasks",
        "edit",
        &one,
        "--title",
        "Get two quotes",
        "--assignee",
        "Ada",
    ]);
    env.settled();
    let task = graph.task("L-tasks", "T1").expect("task");
    assert_eq!(task["title"], "Get two quotes");
    assert_eq!(graph.extension("T1").expect("ours")["assignee"], "Ada");
}

#[tokio::test]
async fn undo_takes_an_assignment_back_unless_it_changed_since() {
    let mut env = Env::new();
    let graph = graph_with(
        &mut env,
        vec![
            task("T1", "Get the quote", "W/\"1\""),
            task("T2", "Chase the invoice", "W/\"1\""),
        ],
        vec![("T1", ours(json!({ "myDay": TODAY })))],
    )
    .await;
    start_on_today(&env);
    env.synced();
    let one = env.local_id(&["tasks", "list"], "T1");
    let two = env.local_id(&["tasks", "list"], "T2");

    env.json(&["tasks", "edit", &one, "--assignee", "Sam"]);
    env.settled();
    let undone = env.json(&["undo"]);
    assert!(undone["refused"].is_null(), "{undone}");
    env.settled();
    let extension = graph.extension("T1").expect("My Day kept");
    assert!(extension.get("assignee").is_none(), "{extension}");
    assert!(extension.get("assigneeStatusSet").is_none(), "{extension}");
    assert_eq!(extension["myDay"], TODAY);
    assert_eq!(status(&graph, "T1"), "notStarted");

    // Undoing a clear assigns it again, waiting.
    env.json(&["tasks", "edit", &one, "--assignee", "Sam"]);
    env.settled();
    let cleared = env.json(&["tasks", "edit", &one, "--clear-assignee"]);
    env.settled();
    env.json(&["undo", cleared["op_id"].as_str().expect("op_id")]);
    env.settled();
    assert_eq!(graph.extension("T1").expect("ours")["assignee"], "Sam");
    assert_eq!(status(&graph, "T1"), "waitingOnOthers");

    // Another machine reassigns T2 after us: undo leaves it alone.
    let assigned = env.json(&["tasks", "edit", &two, "--assignee", "Ada"]);
    env.settled();
    graph.edit(|data| {
        data.extensions.insert(
            "T2".into(),
            ours(json!({ "assignee": "Kim", "assigneeStatusSet": true })),
        );
    });
    elsewhere(&graph, "T2", |_| {});
    env.synced();
    let refused = env.failure(&["undo", assigned["op_id"].as_str().expect("op_id")], 5);
    assert_eq!(refused["error"]["kind"], "conflict");
    assert_eq!(graph.extension("T2").expect("ours")["assignee"], "Kim");
}

#[tokio::test]
async fn an_assignee_set_on_another_machine_arrives_by_sync() {
    let mut env = Env::new();
    let graph = graph_with(
        &mut env,
        vec![task("T1", "Get the quote", "W/\"1\"")],
        Vec::new(),
    )
    .await;
    env.synced();
    assert_eq!(env.json(&["waiting"])["items"], json!([]));

    // Another ms-todo assigns it: our extension and the status, and a new
    // etag, as Graph gives any write.
    graph.edit(|data| {
        data.extensions.insert(
            "T1".into(),
            ours(json!({ "assignee": "Sam", "assigneeStatusSet": true })),
        );
    });
    elsewhere(&graph, "T1", |task| {
        task["status"] = json!("waitingOnOthers")
    });
    env.synced();
    let waiting = env.json(&["waiting"]);
    assert_eq!(titles(&waiting["items"]), ["Get the quote"]);
    assert_eq!(waiting["items"][0]["status"], "waitingOnOthers");

    // And clearing here restores the status the other machine set.
    let id = env.local_id(&["tasks", "list"], "T1");
    env.json(&["tasks", "edit", &id, "--clear-assignee"]);
    env.settled();
    assert_eq!(status(&graph, "T1"), "notStarted");
}
