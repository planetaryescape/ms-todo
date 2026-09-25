//! My Day through the real binary and daemon, against a fake Graph
//! (docs/blueprint/05-custom-features.md#my-day, D-037): `myDay` and
//! `myDayDueSet` in our extension, the due date that mirrors My Day on the
//! phone, the daily rollover, suggestions, undo, and quick add's `+myday`.

mod support;

use serde_json::{Value, json};
use support::Env;
use support::fake_graph::{FakeGraph, list, task};

/// Long past, so any rollover takes it out.
const OLD_DAY: &str = "2026-01-05";

fn due(day: &str) -> Value {
    json!({ "dateTime": format!("{day}T00:00:00.0000000"), "timeZone": "UTC" })
}

fn with(mut task: Value, extra: Value) -> Value {
    if let (Some(task), Some(extra)) = (task.as_object_mut(), extra.as_object()) {
        task.extend(extra.clone());
    }
    task
}

fn my_day_extension(day: &str, due_set: bool) -> Value {
    let mut extension = json!({
        "extensionName": "com.planetaryescape.mstodo",
        "id": "microsoft.graph.openTypeExtension.com.planetaryescape.mstodo",
        "myDay": day
    });
    if due_set {
        extension["myDayDueSet"] = json!(true);
    }
    extension
}

/// Graph with the default list holding `tasks` (and `extensions` by task
/// ID), taking task writes as Graph does.
async fn graph_with(env: &mut Env, tasks: Vec<Value>, extensions: Vec<(&str, Value)>) -> FakeGraph {
    let graph = FakeGraph::start(env, vec![list("L-tasks", "Tasks", "defaultList")]).await;
    graph.edit(|data| {
        data.tasks.insert("L-tasks".into(), tasks);
        for (id, extension) in extensions {
            data.extensions.insert(id.to_owned(), extension);
        }
    });
    graph.accept_task_patches().await;
    graph.accept_moves().await;
    graph
}

/// My Day's day as the daemon has it.
fn today(env: &Env) -> String {
    env.json(&["doctor"])["my_day"]["date"]
        .as_str()
        .expect("my day's date")
        .to_owned()
}

/// (Re)start the daemon with My Day's day fixed at `day` (a debug-build
/// hook), so a test never depends on the clock.
fn start_on(env: &Env, day: &str) {
    env.json(&["daemon", "stop"]);
    env.cmd()
        .env("MS_TODO_MY_DAY_TODAY", day)
        .args(["--format", "json", "daemon", "start"])
        .assert()
        .success();
}

/// Wait up to 20 seconds for the daemon's first rollover to be recorded.
fn rolled_over(env: &Env) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let doctor = env.json(&["doctor"]);
        if let Some(day) = doctor["my_day"]["last_rollover"].as_str() {
            return day.to_owned();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the rollover never ran: {}",
            doctor["my_day"]
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

fn graph_task(graph: &FakeGraph, id: &str) -> Value {
    graph.task("L-tasks", id).expect("task in Graph")
}

fn titles(items: &Value) -> Vec<String> {
    items
        .as_array()
        .expect("items")
        .iter()
        .map(|item| item["title"].as_str().unwrap_or_default().to_owned())
        .collect()
}

fn command_of(env: &Env, action: &str) -> String {
    let found = env
        .outbox()
        .into_iter()
        .find(|op| op["action"] == action)
        .and_then(|op| op["command_id"].as_str().map(str::to_owned));
    assert!(found.is_some(), "no {action} in the outbox");
    found.unwrap_or_default()
}

#[tokio::test]
async fn adding_a_task_with_no_due_date_makes_it_due_today_and_removing_takes_that_away() {
    let mut env = Env::new();
    let graph = graph_with(
        &mut env,
        vec![task("T1", "Call the bank", "W/\"1\"")],
        Vec::new(),
    )
    .await;
    env.synced();
    rolled_over(&env);
    let today = today(&env);
    let id = env.local_id(&["tasks", "list"], "T1");

    let added = env.json(&["myday", "add", &id]);
    assert_eq!(added["action"], "my_day_add");
    assert_eq!(added["items"].as_array().map(Vec::len), Some(1), "{added}");
    assert_eq!(added["items"][0]["extensions"][0]["myDay"], today.as_str());
    env.settled();
    let extension = graph.extension("T1").expect("extension");
    assert_eq!(extension["myDay"], today.as_str());
    assert_eq!(extension["myDayDueSet"], true);
    let due = graph_task(&graph, "T1")["dueDateTime"].clone();
    assert_eq!(due["dateTime"], format!("{today}T00:00:00"));
    assert_eq!(due["timeZone"], "Europe/London");
    assert_eq!(
        titles(&env.json(&["myday", "list"])["items"]),
        ["Call the bank"]
    );
    assert_eq!(
        titles(&env.json(&["tasks", "list", "--my-day"])["items"]),
        ["Call the bank"]
    );
    // Adding again changes nothing.
    let again = env.json(&["myday", "add", &id]);
    assert_eq!(again["items"], json!([]));

    let removed = env.json(&["myday", "remove", &id]);
    assert_eq!(removed["action"], "my_day_remove");
    env.settled();
    assert_eq!(graph.extension("T1"), None, "nothing of ours was left");
    assert!(graph_task(&graph, "T1")["dueDateTime"].is_null());
    assert_eq!(env.json(&["myday", "list"])["items"], json!([]));
}

#[tokio::test]
async fn a_due_date_of_the_users_is_kept_in_and_out_of_my_day() {
    let mut env = Env::new();
    let dated = with(
        task("T1", "Pay rent", "W/\"1\""),
        json!({ "dueDateTime": due("2026-12-01") }),
    );
    let theirs = json!({ "extensionName": "com.planetaryescape.mstodo", "keep": "me" });
    let graph = graph_with(&mut env, vec![dated], vec![("T1", theirs)]).await;
    env.synced();
    rolled_over(&env);
    let id = env.local_id(&["tasks", "list"], "T1");

    env.json(&["myday", "add", &id]);
    env.settled();
    let extension = graph.extension("T1").expect("extension");
    assert_eq!(extension["keep"], "me", "the merge kept another field");
    assert!(extension.get("myDayDueSet").is_none());
    assert_eq!(
        graph_task(&graph, "T1")["dueDateTime"],
        due("2026-12-01"),
        "no PATCH of the due date"
    );

    env.json(&["myday", "remove", &id]);
    env.settled();
    assert_eq!(graph.extension("T1").expect("extension")["keep"], "me");
    assert!(
        graph
            .extension("T1")
            .expect("extension")
            .get("myDay")
            .is_none()
    );
    assert_eq!(graph_task(&graph, "T1")["dueDateTime"], due("2026-12-01"));
}

#[tokio::test]
async fn removing_keeps_a_due_date_changed_since_my_day_set_it() {
    let mut env = Env::new();
    let graph = graph_with(
        &mut env,
        vec![task("T1", "Call the bank", "W/\"1\"")],
        Vec::new(),
    )
    .await;
    env.synced();
    rolled_over(&env);
    let id = env.local_id(&["tasks", "list"], "T1");
    env.json(&["myday", "add", &id]);
    env.settled();
    // The phone moves the due date.
    graph.edit(|data| {
        let task = data
            .tasks
            .get_mut("L-tasks")
            .and_then(|tasks| tasks.first_mut())
            .expect("task");
        task["dueDateTime"] = due("2026-11-20");
        task["@odata.etag"] = json!("W/\"phone\"");
    });
    env.synced();

    env.json(&["myday", "remove", &id]);
    env.settled();
    assert!(graph.extension("T1").is_none());
    assert_eq!(graph_task(&graph, "T1")["dueDateTime"], due("2026-11-20"));
}

#[tokio::test]
async fn the_rollover_catches_up_once_clearing_only_what_my_day_set_on_open_tasks() {
    let mut env = Env::new();
    let tasks = vec![
        with(
            task("T-set", "Set by My Day", "W/\"1\""),
            json!({ "dueDateTime": due(OLD_DAY) }),
        ),
        with(
            task("T-user", "Moved by the user", "W/\"1\""),
            json!({ "dueDateTime": due("2026-12-01") }),
        ),
        with(
            task("T-done", "Finished", "W/\"1\""),
            json!({ "status": "completed", "dueDateTime": due(OLD_DAY) }),
        ),
        task("T-other", "Not in My Day", "W/\"1\""),
    ];
    let extensions = vec![
        ("T-set", my_day_extension(OLD_DAY, true)),
        ("T-user", my_day_extension(OLD_DAY, true)),
        ("T-done", my_day_extension(OLD_DAY, true)),
    ];
    let graph = graph_with(&mut env, tasks, extensions).await;
    env.synced();
    // The daemon was off since OLD_DAY: it rolls over once, now.
    let last = rolled_over(&env);
    assert_eq!(last, today(&env));
    env.settled();

    for id in ["T-set", "T-user", "T-done"] {
        assert!(graph.extension(id).is_none(), "{id} left My Day");
    }
    assert!(graph_task(&graph, "T-set")["dueDateTime"].is_null());
    assert_eq!(
        graph_task(&graph, "T-user")["dueDateTime"],
        due("2026-12-01")
    );
    assert_eq!(graph_task(&graph, "T-done")["dueDateTime"], due(OLD_DAY));
    let rollover = command_of(&env, "my_day_rollover");
    let ops = env
        .outbox()
        .into_iter()
        .filter(|op| op["command_id"] == rollover.as_str())
        .count();
    assert_eq!(ops, 4, "an extension write each, and one due date");

    // Again: nothing to do.
    let again = env.json(&["myday", "rollover", "--dry-run"]);
    assert_eq!(again["targets"], Value::Null, "{again}");
    let writes = graph.writes().await.len();
    let ran = env.json(&["myday", "rollover"]);
    assert_eq!(ran["items"], json!([]));
    env.settled();
    assert_eq!(graph.writes().await.len(), writes);

    // What it left open is suggested.
    let suggested = env.json(&["myday", "suggest"])["items"].clone();
    let mut left: Vec<(String, String)> = suggested
        .as_array()
        .expect("items")
        .iter()
        .map(|task| {
            (
                task["title"].as_str().unwrap_or_default().to_owned(),
                task["suggestion"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    left.sort();
    assert_eq!(
        left,
        [
            ("Moved by the user".to_owned(), "left_over".to_owned()),
            ("Set by My Day".to_owned(), "left_over".to_owned()),
        ]
    );
    assert_eq!(suggested[0]["left_from"], OLD_DAY);

    // One undo puts the rollover back.
    let undone = env.json(&["undo", &rollover]);
    assert_eq!(undone["undoes"], rollover.as_str());
    env.settled();
    assert_eq!(graph.extension("T-set").expect("back")["myDay"], OLD_DAY);
    assert_eq!(graph.extension("T-set").expect("back")["myDayDueSet"], true);
    let restored = graph_task(&graph, "T-set")["dueDateTime"]["dateTime"].clone();
    assert!(
        restored
            .as_str()
            .is_some_and(|at| at.starts_with(&format!("{OLD_DAY}T00:00:00"))),
        "{restored}"
    );
}

#[tokio::test]
async fn a_dry_run_shows_the_plan_and_a_task_put_back_elsewhere_is_left_there() {
    let mut env = Env::new();
    let graph = graph_with(
        &mut env,
        vec![task("T1", "Call the bank", "W/\"1\"")],
        Vec::new(),
    )
    .await;
    env.synced();
    rolled_over(&env);
    // Put in an earlier My Day after today's rollover (another ms-todo, or
    // the daemon off at midnight).
    graph.edit(|data| {
        data.extensions
            .insert("T1".into(), my_day_extension(OLD_DAY, true));
        let task = data
            .tasks
            .get_mut("L-tasks")
            .and_then(|tasks| tasks.first_mut())
            .expect("task");
        task["dueDateTime"] = due(OLD_DAY);
        task["@odata.etag"] = json!("W/\"2\"");
    });
    env.synced();

    let plan = env.json(&["myday", "rollover", "--dry-run"]);
    assert_eq!(plan["dry_run"], true);
    assert_eq!(plan["action"], "my_day_rollover");
    assert_eq!(titles(&plan["targets"]), ["Call the bank"]);
    let id = env.local_id(&["tasks", "list"], "T1");
    assert_eq!(plan["changes"]["due_cleared"], json!([id]));
    let table = env
        .cmd()
        .args(["--format", "table", "myday", "rollover", "--dry-run"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let table = String::from_utf8(table).expect("utf-8");
    assert!(table.contains("\"Call the bank\""), "{table}");

    // Before the rollover reaches Graph, another ms-todo puts it in
    // today's My Day: the rollover leaves it there.
    let today = today(&env);
    graph.edit(|data| {
        data.extensions
            .insert("T1".into(), my_day_extension(&today, true));
        let task = data
            .tasks
            .get_mut("L-tasks")
            .and_then(|tasks| tasks.first_mut())
            .expect("task");
        task["dueDateTime"] = due(&today);
    });
    env.json(&["myday", "rollover"]);
    env.settled();
    assert_eq!(
        graph.extension("T1").expect("kept")["myDay"],
        today.as_str()
    );
    assert_eq!(graph_task(&graph, "T1")["dueDateTime"], due(&today));
}

#[tokio::test]
async fn undo_refuses_what_changed_since_and_reverses_the_rest() {
    let mut env = Env::new();
    let graph = graph_with(
        &mut env,
        vec![
            task("T1", "Call the bank", "W/\"1\""),
            task("T2", "Book dentist", "W/\"1\""),
        ],
        Vec::new(),
    )
    .await;
    env.synced();
    rolled_over(&env);
    let one = env.local_id(&["tasks", "list"], "T1");
    let two = env.local_id(&["tasks", "list"], "T2");
    let added = env.json(&["myday", "add", &one, &two]);
    let op = added["op_id"].as_str().expect("op_id").to_owned();
    env.settled();
    // The phone gives T2 a due date of its own.
    graph.edit(|data| {
        let task = data
            .tasks
            .get_mut("L-tasks")
            .and_then(|tasks| tasks.iter_mut().find(|task| task["id"] == "T2"))
            .expect("task");
        task["dueDateTime"] = due("2026-11-20");
        task["@odata.etag"] = json!("W/\"phone\"");
    });
    env.synced();

    let undone = env.json(&["undo", &op]);
    let refused = undone["refused"].as_array().expect("refused");
    assert_eq!(refused.len(), 1, "{undone}");
    assert_eq!(refused[0]["title"], "Book dentist");
    env.settled();
    assert!(graph.extension("T1").is_none());
    assert!(graph_task(&graph, "T1")["dueDateTime"].is_null());
    // T2 left My Day, but its new due date stays.
    assert!(
        graph
            .extension("T2")
            .is_none_or(|extension| extension.get("myDay").is_none())
    );
    assert_eq!(graph_task(&graph, "T2")["dueDateTime"], due("2026-11-20"));

    // Undoing a remove puts the task back in My Day.
    env.json(&["myday", "add", &one]);
    env.settled();
    let removed = env.json(&["myday", "remove", &one]);
    env.settled();
    env.json(&["undo", removed["op_id"].as_str().expect("op_id")]);
    env.settled();
    assert_eq!(
        graph.extension("T1").expect("back")["myDay"],
        today(&env).as_str()
    );
    assert!(!graph_task(&graph, "T1")["dueDateTime"].is_null());
}

#[tokio::test]
async fn suggestions_are_due_today_then_overdue_and_never_in_my_day() {
    let mut env = Env::new();
    let graph = graph_with(&mut env, Vec::new(), Vec::new()).await;
    env.synced();
    rolled_over(&env);
    let today = today(&env);
    graph.edit(|data| {
        data.tasks.insert(
            "L-tasks".into(),
            vec![
                with(
                    task("T-late", "Late", "W/\"1\""),
                    json!({ "dueDateTime": due(OLD_DAY) }),
                ),
                with(
                    task("T-now", "Now", "W/\"1\""),
                    json!({ "dueDateTime": due(&today) }),
                ),
                with(
                    task("T-in", "Already in", "W/\"1\""),
                    json!({ "dueDateTime": due(&today) }),
                ),
                with(
                    task("T-done", "Done", "W/\"1\""),
                    json!({ "status": "completed", "dueDateTime": due(OLD_DAY) }),
                ),
                with(
                    task("T-later", "Later", "W/\"1\""),
                    json!({ "dueDateTime": due("2099-01-01") }),
                ),
            ],
        );
        data.extensions
            .insert("T-in".into(), my_day_extension(&today, false));
    });
    env.synced();
    let suggested = env.json(&["myday", "suggest"])["items"].clone();
    assert_eq!(titles(&suggested), ["Now", "Late"]);
    assert_eq!(suggested[0]["suggestion"], "due_today");
    assert_eq!(suggested[1]["suggestion"], "overdue");
    assert_eq!(suggested[0]["list"], "Tasks");
    let csv = env
        .cmd()
        .args(["--format", "csv", "myday", "suggest"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let csv = String::from_utf8(csv).expect("utf-8");
    assert!(
        csv.starts_with("id,title,list,suggestion,due,left_from\n"),
        "{csv}"
    );
}

#[tokio::test]
async fn quick_add_puts_a_task_in_my_day_in_the_create_itself() {
    let mut env = Env::new();
    let graph = graph_with(&mut env, Vec::new(), Vec::new()).await;
    env.synced();
    rolled_over(&env);
    let today = today(&env);

    let parsed = env.json(&["tasks", "parse", "Call the bank +myday"]);
    assert_eq!(parsed["my_day"], true);
    assert_eq!(parsed["warnings"], json!([]));
    let added = env.json(&["tasks", "add", "Call the bank +myday"]);
    assert_eq!(added["items"][0]["title"], "Call the bank");
    env.settled();
    let created = graph.tasks_in("L-tasks");
    assert_eq!(created.len(), 1);
    let id = created[0]["id"].as_str().expect("id");
    assert_eq!(
        created[0]["dueDateTime"]["dateTime"],
        format!("{today}T00:00:00")
    );
    let extension = graph.extension(id).expect("extension");
    assert_eq!(extension["myDay"], today.as_str());
    assert_eq!(extension["myDayDueSet"], true);
    assert!(extension["opId"].is_string());
    // One POST, nothing else.
    assert_eq!(graph.writes().await.len(), 1);

    // --my-day with a due date keeps the date.
    env.json(&[
        "tasks",
        "add",
        "Pay rent",
        "--due",
        "2026-12-01",
        "--my-day",
    ]);
    env.settled();
    let rent = graph
        .tasks_in("L-tasks")
        .into_iter()
        .find(|task| task["title"] == "Pay rent")
        .expect("rent");
    let rent_extension = graph
        .extension(rent["id"].as_str().expect("id"))
        .expect("extension");
    assert_eq!(rent_extension["myDay"], today.as_str());
    assert!(rent_extension.get("myDayDueSet").is_none());
    assert_eq!(
        titles(&env.json(&["myday", "list"])["items"]),
        ["Call the bank", "Pay rent"]
    );
}

#[tokio::test]
async fn doctor_reports_my_day_and_the_phone_setting_and_a_bad_rollover_time() {
    let mut env = Env::new();
    let config = env.home.path().join("config/ms-todo");
    std::fs::create_dir_all(&config).expect("config dir");
    std::fs::write(
        config.join("config.toml"),
        "[my_day]\nrollover_time = \"25:00\"\n",
    )
    .expect("config");
    let _graph = graph_with(&mut env, Vec::new(), Vec::new()).await;
    env.synced();
    let doctor = env.json(&["doctor"]);
    assert_eq!(doctor["my_day"]["rollover_time"], "00:00");
    assert!(
        doctor["my_day"]["phone"]
            .as_str()
            .is_some_and(|phone| phone.contains("Show 'Due Today' tasks in My Day")),
        "{doctor}"
    );
    let problems = doctor["problems"].to_string();
    assert!(problems.contains("my_day.rollover_time"), "{problems}");
}

#[tokio::test]
async fn a_plain_undo_takes_back_your_last_change_not_the_automatic_rollover() {
    let mut env = Env::new();
    let graph = graph_with(
        &mut env,
        vec![
            task("T1", "Buy milk", "W/\"1\""),
            task("T2", "Call the bank", "W/\"1\""),
        ],
        Vec::new(),
    )
    .await;
    start_on(&env, "2026-09-20");
    env.synced();
    rolled_over(&env);
    let milk = env.local_id(&["tasks", "list"], "T1");
    let edited = env.json(&["tasks", "edit", &milk, "--title", "Buy oat milk"]);
    let edit = edited["op_id"].as_str().expect("op_id").to_owned();
    env.settled();

    // T2 is in an earlier day's My Day, and the daemon restarts a day on.
    graph.edit(|data| {
        data.extensions
            .insert("T2".into(), my_day_extension(OLD_DAY, false));
        let task = data
            .tasks
            .get_mut("L-tasks")
            .and_then(|tasks| tasks.iter_mut().find(|task| task["id"] == "T2"))
            .expect("task");
        task["@odata.etag"] = json!("W/\"2\"");
    });
    env.synced();
    start_on(&env, "2026-09-21");
    env.synced();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while graph.extension("T2").is_some() {
        assert!(
            std::time::Instant::now() < deadline,
            "the automatic rollover never took T2 out"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    env.settled();
    let rollover = command_of(&env, "my_day_rollover");

    let undone = env.json(&["undo"]);
    assert_eq!(undone["undoes"], edit.as_str());
    env.settled();
    assert_eq!(graph_task(&graph, "T1")["title"], "Buy milk");
    assert!(graph.extension("T2").is_none(), "the rollover stays");

    // By name, the rollover can still be undone.
    let by_name = env.json(&["undo", &rollover]);
    assert_eq!(by_name["undoes"], rollover.as_str());
    env.settled();
    assert_eq!(graph.extension("T2").expect("back")["myDay"], OLD_DAY);
}

/// The phone sets a due date on My Day's day, 2026-09-20, right after the
/// extension write reads the task back: ms-todo's due-date edit is skipped.
fn phone_sets_t1(data: &mut support::fake_graph::Data) {
    phone_sets(data, "T1");
}

fn phone_sets_t2(data: &mut support::fake_graph::Data) {
    phone_sets(data, "T2");
}

fn phone_sets(data: &mut support::fake_graph::Data, id: &str) {
    if let Some(task) = data
        .tasks
        .get_mut("L-tasks")
        .and_then(|tasks| tasks.iter_mut().find(|task| task["id"] == id))
    {
        task["dueDateTime"] = due("2026-09-20");
    }
}

#[tokio::test]
async fn a_due_date_the_phone_set_while_adding_is_never_taken_away() {
    let mut env = Env::new();
    let graph = graph_with(
        &mut env,
        vec![
            task("T1", "Call the bank", "W/\"1\""),
            task("T2", "Book dentist", "W/\"1\""),
        ],
        Vec::new(),
    )
    .await;
    // The extension write reads the task, writes, and reads it again; the
    // phone's change lands after that, before the due-date edit's read.
    graph
        .edit_after_gets(r"^/v1\.0/me/todo/lists/L-tasks/tasks/T1$", 2, phone_sets_t1)
        .await;
    graph
        .edit_after_gets(r"^/v1\.0/me/todo/lists/L-tasks/tasks/T2$", 2, phone_sets_t2)
        .await;
    start_on(&env, "2026-09-20");
    env.synced();
    rolled_over(&env);
    let one = env.local_id(&["tasks", "list"], "T1");
    let two = env.local_id(&["tasks", "list"], "T2");
    env.json(&["myday", "add", &one, &two]);
    env.settled();
    for id in ["T1", "T2"] {
        let extension = graph.extension(id).expect("in My Day");
        assert_eq!(extension["myDay"], "2026-09-20");
        assert!(extension.get("myDayDueSet").is_none(), "{id}: {extension}");
        assert_eq!(graph_task(&graph, id)["dueDateTime"], due("2026-09-20"));
    }
    let skipped = env
        .outbox()
        .into_iter()
        .filter(|op| {
            op["state"] == "done"
                && op["note"]
                    .as_str()
                    .is_some_and(|note| note.starts_with("skipped:"))
        })
        .count();
    assert_eq!(skipped, 4, "each task's due-date edit and flag");

    // Removing keeps the phone's date.
    env.json(&["myday", "remove", &one]);
    env.settled();
    assert!(graph.extension("T1").is_none());
    assert_eq!(graph_task(&graph, "T1")["dueDateTime"], due("2026-09-20"));

    // So does the rollover, a day on.
    start_on(&env, "2026-09-21");
    env.synced();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while graph.extension("T2").is_some() {
        assert!(
            std::time::Instant::now() < deadline,
            "the rollover never took T2 out"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    env.settled();
    assert_eq!(graph_task(&graph, "T2")["dueDateTime"], due("2026-09-20"));
}

/// The phone sets My Day's day as T1's due date right after ms-todo's
/// due-date edit has checked it, so the edit's PATCH meets a new etag.
fn phone_sets_t1_and_moves_its_etag(data: &mut support::fake_graph::Data) {
    phone_sets(data, "T1");
    if let Some(task) = data
        .tasks
        .get_mut("L-tasks")
        .and_then(|tasks| tasks.iter_mut().find(|task| task["id"] == "T1"))
    {
        task["@odata.etag"] = json!("W/\"phone\"");
    }
}

#[tokio::test]
async fn a_due_date_the_phone_set_just_before_ours_landed_is_never_claimed() {
    let mut env = Env::new();
    let graph = graph_with(
        &mut env,
        vec![task("T1", "Call the bank", "W/\"1\"")],
        Vec::new(),
    )
    .await;
    graph.refuse_stale_task_patches().await;
    // The extension write reads the task twice; the due-date edit's check
    // is the third read, and the phone lands after it: the PATCH is a 412
    // and Graph already has the same day, set by the phone.
    graph
        .edit_after_gets(
            r"^/v1\.0/me/todo/lists/L-tasks/tasks/T1$",
            3,
            phone_sets_t1_and_moves_its_etag,
        )
        .await;
    start_on(&env, "2026-09-20");
    env.synced();
    rolled_over(&env);
    let id = env.local_id(&["tasks", "list"], "T1");
    env.json(&["myday", "add", &id]);
    env.settled();
    let extension = graph.extension("T1").expect("in My Day");
    assert_eq!(extension["myDay"], "2026-09-20");
    assert!(extension.get("myDayDueSet").is_none(), "{extension}");
    let skipped = env
        .outbox()
        .into_iter()
        .filter(|op| {
            op["note"]
                .as_str()
                .is_some_and(|note| note.starts_with("skipped:"))
        })
        .count();
    assert_eq!(skipped, 2, "the due-date edit and the flag");

    // Removing keeps the phone's date.
    env.json(&["myday", "remove", &id]);
    env.settled();
    assert_eq!(graph_task(&graph, "T1")["dueDateTime"], due("2026-09-20"));
}
