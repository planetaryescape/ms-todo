//! Steps and links (rung 8a, D-055) through the real binary and daemon,
//! against a fake Graph that behaves as S1, S6, S14 and S15 found: what
//! reaches Graph (bodies, `If-Match` with the task's etag, `isChecked` on
//! every step PATCH), what the cache shows before and after, undo, a lost
//! answer to a create, the one-link rule, and a step checked on the phone
//! arriving by delta.

mod support;

use serde_json::{Value, json};
use support::Env;
use support::fake_graph::{FakeGraph, list, task};
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, ResponseTemplate};

const STEPS: &str = "checklistItems";
const LINKS: &str = "linkedResources";

/// Graph with the default list holding "Paint the room", which has no
/// steps, and the fake answering step and link writes.
async fn graph(env: &mut Env) -> FakeGraph {
    let graph = FakeGraph::start(env, vec![list("L-tasks", "Tasks", "defaultList")]).await;
    graph.edit(|data| {
        data.tasks.insert(
            "L-tasks".into(),
            vec![task("T1", "Paint the room", "W/\"e1\"")],
        );
    });
    graph.accept_children().await;
    env.synced();
    graph
}

fn paint(env: &Env) -> String {
    env.local_id(&["tasks", "list"], "T1")
}

fn steps(env: &Env, task: &str) -> Vec<Value> {
    env.json(&["steps", "list", task])["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn names(steps: &[Value]) -> Vec<(String, bool)> {
    steps
        .iter()
        .map(|step| {
            (
                step["displayName"].as_str().unwrap_or_default().to_owned(),
                step["isChecked"] == true,
            )
        })
        .collect()
}

/// The child writes Graph got, as `(method, path after the task, body,
/// If-Match)`.
async fn child_writes(graph: &FakeGraph) -> Vec<(String, String, Value, Option<String>)> {
    graph
        .writes()
        .await
        .into_iter()
        .map(|request| {
            let path = request.url.path();
            let after = path.split("/tasks/T1").nth(1).unwrap_or(path).to_owned();
            let body = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
            let if_match = request
                .headers
                .get("if-match")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            (request.method.to_string(), after, body, if_match)
        })
        .collect()
}

fn graph_steps(graph: &FakeGraph) -> Vec<(String, bool)> {
    names(&graph.children("L-tasks", "T1", STEPS))
}

#[tokio::test]
async fn several_steps_are_added_in_order_and_show_at_once() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    let task = paint(&env);

    let added = env.json(&[
        "steps",
        "add",
        &task,
        "Buy paint",
        "Tape the edges",
        "Paint",
    ]);
    assert_eq!(added["action"], "step_add");
    let shown = &added["items"][0];
    assert_eq!(shown["sync_state"], "pending");
    assert_eq!(
        names(shown[STEPS].as_array().expect("steps")),
        [
            ("Buy paint".to_owned(), false),
            ("Tape the edges".to_owned(), false),
            ("Paint".to_owned(), false)
        ],
        "in the cache before Graph has them"
    );

    env.settled();
    let sent = child_writes(&graph).await;
    let posts: Vec<&Value> = sent
        .iter()
        .filter(|(verb, path, ..)| verb == "POST" && path == "/checklistItems")
        .map(|(_, _, body, _)| &body["displayName"])
        .collect();
    assert_eq!(
        posts,
        [
            &json!("Buy paint"),
            &json!("Tape the edges"),
            &json!("Paint")
        ]
    );
    assert_eq!(graph_steps(&graph).len(), 3);
    let listed = steps(&env, &task);
    assert_eq!(listed[0]["index"], 1);
    assert!(
        listed.iter().all(|step| step["id"]
            .as_str()
            .is_some_and(|id| id.starts_with("checklistItems-"))),
        "Graph's IDs replaced the placeholders: {listed:?}"
    );
    assert_eq!(
        env.json(&["tasks", "list"])["items"][0]["sync_state"],
        "synced"
    );
}

#[tokio::test]
async fn check_edit_and_delete_send_is_checked_and_the_tasks_etag() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    let task = paint(&env);
    env.json(&["steps", "add", &task, "Buy paint", "Tape"]);
    env.settled();
    let etag = |graph: &FakeGraph| {
        let mut etag = None;
        graph.edit(|data| {
            etag = data.tasks["L-tasks"][0]["@odata.etag"]
                .as_str()
                .map(str::to_owned)
        });
        etag
    };
    let before = child_writes(&graph).await.len();

    let current = etag(&graph);
    env.json(&["steps", "check", &task, "Buy paint"]);
    env.settled();
    let checked = etag(&graph);
    env.json(&["steps", "edit", &task, "1", "Buy white paint"]);
    env.settled();
    let edited = etag(&graph);
    env.json(&["steps", "uncheck", &task, "1"]);
    env.settled();

    let sent = child_writes(&graph).await[before..].to_vec();
    assert_eq!(sent.len(), 3, "{sent:?}");
    assert_eq!(sent[0].0, "PATCH");
    assert_eq!(sent[0].2, json!({ "isChecked": true }));
    assert_eq!(sent[0].3, current, "If-Match is the task's etag");
    // A rename carries isChecked, or Graph would uncheck the step (S1).
    assert_eq!(
        sent[1].2,
        json!({ "displayName": "Buy white paint", "isChecked": true })
    );
    assert_eq!(sent[1].3, checked, "the etag read back after the check");
    assert_eq!(sent[2].2, json!({ "isChecked": false }));
    assert_eq!(sent[2].3, edited);
    assert_eq!(
        graph_steps(&graph),
        [
            ("Buy white paint".to_owned(), false),
            ("Tape".to_owned(), false)
        ]
    );

    // Deleting needs --yes off a terminal.
    env.failure(&["steps", "delete", &task, "Tape"], 2);
    env.json(&["steps", "delete", &task, "Tape", "--yes"]);
    env.settled();
    let last = child_writes(&graph).await.pop().expect("a write");
    assert_eq!(last.0, "DELETE");
    assert!(last.3.is_some(), "a delete sends If-Match too");
    assert_eq!(graph_steps(&graph), [("Buy white paint".to_owned(), false)]);
    assert_eq!(names(&steps(&env, &task)), graph_steps(&graph));
}

#[tokio::test]
async fn a_step_is_named_by_number_id_or_text_and_a_bad_name_changes_nothing() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    let task = paint(&env);
    env.json(&["steps", "add", &task, "Buy paint", "Tape", "Tape"]);
    env.settled();
    let writes = child_writes(&graph).await.len();

    let ambiguous = env.failure(&["steps", "check", &task, "Tape"], 2);
    assert!(
        ambiguous["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("(2, 3)"),
        "{ambiguous}"
    );
    env.failure(&["steps", "check", &task, "1", "9"], 3);
    assert_eq!(child_writes(&graph).await.len(), writes, "nothing sent");

    let id = steps(&env, &task)[2]["id"].as_str().expect("id").to_owned();
    let dry = env.json(&["steps", "check", &task, &id, "Buy paint", "--dry-run"]);
    assert_eq!(dry["dry_run"], true);
    assert_eq!(dry["changes"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        child_writes(&graph).await.len(),
        writes,
        "a dry run sends nothing"
    );
}

#[tokio::test]
async fn undo_reverses_steps_unless_they_changed_since() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    let task = paint(&env);
    let added = env.json(&["steps", "add", &task, "Buy paint", "Tape"]);
    env.settled();
    let checked = env.json(&["steps", "check", &task, "1", "2"]);
    env.settled();

    let undone = env.json(&["undo"]);
    assert_eq!(undone["undoes"], checked["op_id"]);
    env.settled();
    assert_eq!(
        graph_steps(&graph),
        [("Buy paint".to_owned(), false), ("Tape".to_owned(), false)]
    );

    // Undoing the add deletes both steps, unless one changed since: here
    // "Tape" is checked on the phone first, so it's left alone.
    graph.edit(|data| {
        let task = &mut data.tasks.get_mut("L-tasks").expect("list")[0];
        task[STEPS][1]["isChecked"] = json!(true);
        task["@odata.etag"] = json!("W/\"phone\"");
    });
    env.synced();
    let undone = env.json(&["undo", added["op_id"].as_str().expect("op_id")]);
    assert_eq!(undone["refused"][0]["id"], task);
    assert!(
        undone["refused"][0]["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("\"Tape\""),
        "{undone}"
    );
    env.settled();
    assert_eq!(graph_steps(&graph), [("Tape".to_owned(), true)]);
}

#[tokio::test]
async fn undoing_a_delete_adds_the_step_back_checked() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    let task = paint(&env);
    env.json(&["steps", "add", &task, "Buy paint"]);
    env.settled();
    env.json(&["steps", "check", &task, "1"]);
    env.settled();
    env.json(&["steps", "delete", &task, "1", "--yes"]);
    env.settled();
    assert!(graph_steps(&graph).is_empty());

    env.json(&["undo"]);
    env.settled();
    let back = graph.children("L-tasks", "T1", STEPS);
    assert_eq!(names(&back), [("Buy paint".to_owned(), true)]);
    assert!(back[0].get("checkedDateTime").is_some(), "{back:?}");
}

#[tokio::test]
async fn a_step_changed_on_the_phone_meanwhile_is_resent_or_refused() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    let task = paint(&env);
    env.json(&["steps", "add", &task, "Buy paint", "Tape"]);
    env.settled();

    // The phone checks the other step: the etag moves, our check of step
    // 1 gets a 412, and it's sent again with the new etag.
    graph.edit(|data| {
        let task = &mut data.tasks.get_mut("L-tasks").expect("list")[0];
        task[STEPS][1]["isChecked"] = json!(true);
        task["@odata.etag"] = json!("W/\"phone-1\"");
    });
    let checked = env.json(&["steps", "check", &task, "1"]);
    env.op_in_state(checked["op_id"].as_str().expect("op_id"), "done");
    assert_eq!(
        graph_steps(&graph),
        [("Buy paint".to_owned(), true), ("Tape".to_owned(), true)]
    );

    // The phone renames step 2 while we rename it too: nothing is
    // overwritten, and ours is rolled back.
    env.synced();
    graph.edit(|data| {
        let task = &mut data.tasks.get_mut("L-tasks").expect("list")[0];
        task[STEPS][1]["displayName"] = json!("Masking tape");
        task["@odata.etag"] = json!("W/\"phone-2\"");
    });
    let renamed = env.json(&["steps", "edit", &task, "2", "Blue tape"]);
    let op = env.op_in_state(renamed["op_id"].as_str().expect("op_id"), "failed");
    assert_eq!(op["last_error"]["kind"], "conflict");
    assert_eq!(graph_steps(&graph)[1], ("Masking tape".to_owned(), true));
    env.synced();
    assert_eq!(
        names(&steps(&env, &task))[1],
        ("Masking tape".to_owned(), true)
    );
}

#[tokio::test]
async fn a_step_create_with_no_answer_stays_unknown_and_is_never_resent() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    let task = paint(&env);
    Mock::given(method("POST"))
        .and(path_regex(r"/checklistItems$"))
        .respond_with(ResponseTemplate::new(503).set_body_json(
            json!({ "error": { "code": "ServiceUnavailable", "message": "try later" } }),
        ))
        .mount(&graph.server)
        .await;

    let added = env.json(&["steps", "add", &task, "Buy paint"]);
    let op_id = added["op_id"].as_str().expect("op_id");
    let op = env.op_in_state(op_id, "unknown");
    assert_eq!(op["flagged"], true, "nothing can find a step but the user");
    assert!(
        op["note"]
            .as_str()
            .unwrap_or_default()
            .contains("outbox retry"),
        "{op}"
    );
    env.synced();
    env.synced();
    let posts = child_writes(&graph).await;
    assert_eq!(posts.len(), 1, "sent once: {posts:?}");
    let shown = env.json(&["tasks", "list"])["items"][0].clone();
    assert_eq!(shown["sync_state"], "unknown");
    assert_eq!(
        names(shown[STEPS].as_array().expect("steps")),
        [("Buy paint".to_owned(), false)]
    );
    env.failure(&["undo"], 2);

    env.json(&["outbox", "discard", op_id, "--yes"]);
    env.synced();
    assert!(steps(&env, &task).is_empty(), "Graph never had it");
}

#[tokio::test]
async fn a_task_takes_one_link_which_is_added_changed_and_deleted() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    let task = paint(&env);

    let added = env.json(&[
        "links",
        "add",
        &task,
        "https://example.com/colours",
        "--name",
        "Colour chart",
    ]);
    assert_eq!(added["action"], "link_add");
    env.settled();
    let sent = child_writes(&graph).await;
    assert_eq!(
        sent[0].2,
        json!({
            "webUrl": "https://example.com/colours",
            "applicationName": "ms-todo",
            "displayName": "Colour chart"
        })
    );

    let second = env.failure(&["links", "add", &task, "https://example.com/other"], 2);
    assert!(
        second["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("allows one per task"),
        "{second}"
    );
    env.failure(&["links", "add", &task, "not a url"], 2);

    env.json(&["links", "edit", &task, "--url", "https://example.com/v2"]);
    env.settled();
    let patch = child_writes(&graph).await.pop().expect("patch");
    assert_eq!(patch.0, "PATCH");
    assert_eq!(patch.2, json!({ "webUrl": "https://example.com/v2" }));
    let listed = env.json(&["links", "list", &task])["items"].clone();
    assert_eq!(listed[0]["webUrl"], "https://example.com/v2");
    assert_eq!(listed[0]["displayName"], "Colour chart");

    env.json(&["undo"]);
    env.settled();
    assert_eq!(
        graph.children("L-tasks", "T1", LINKS)[0]["webUrl"],
        "https://example.com/colours"
    );

    env.failure(&["links", "delete", &task], 2);
    env.json(&["links", "delete", &task, "--yes"]);
    env.settled();
    assert!(graph.children("L-tasks", "T1", LINKS).is_empty());
    assert!(
        env.json(&["links", "list", &task])["items"]
            .as_array()
            .is_some_and(Vec::is_empty)
    );
}

#[tokio::test]
async fn a_step_checked_on_the_phone_arrives_by_delta() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    let task = paint(&env);
    env.json(&["steps", "add", &task, "Buy paint", "Tape", "Paint"]);
    env.settled();
    env.synced();

    graph.edit(|data| {
        let task = &mut data.tasks.get_mut("L-tasks").expect("list")[0];
        task[STEPS][1]["isChecked"] = json!(true);
        task[STEPS][1]["checkedDateTime"] = json!("2026-09-25T09:00:00Z");
        task["@odata.etag"] = json!("W/\"phone\"");
    });
    env.synced();
    assert_eq!(
        names(&steps(&env, &task)),
        [
            ("Buy paint".to_owned(), false),
            ("Tape".to_owned(), true),
            ("Paint".to_owned(), false)
        ]
    );
    let starts = graph.delta_starts("L-tasks").await;
    assert!(
        starts.last().is_some_and(Option::is_some),
        "a delta round, not a whole read: {starts:?}"
    );
}
