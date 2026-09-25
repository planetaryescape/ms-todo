//! List suggestions (rung 6b, D-053) through the real binary and daemon:
//! `tasks suggest-list`, the note after `tasks add`, `doctor`, and the
//! API key from `api_key_command`, against a mock TypeSafe. The lists and
//! tasks here are made up.

mod support;

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use ms_todo_protocol::{Codec, Message, Payload, Request as IpcRequest, Response, ResponseData};
use serde_json::{Value, json};
use support::Env;
use support::fake_graph::{FakeGraph, list, task};
use tokio::net::UnixStream;
use tokio_util::codec::Framed;
use wiremock::matchers::{header, method};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const KEY: &str = "sk-from-command";

/// `[suggest]` on, the key printed by a command run without a shell.
const ENABLED: &str = r#"
[suggest]
enabled = true
min_confidence = 0.8
api_key_command = ["printf", "%s\n", "sk-from-command"]
"#;

fn write_config(env: &Env, contents: &str) {
    let dir = env.home.path().join("config").join("ms-todo");
    std::fs::create_dir_all(&dir).expect("config dir");
    std::fs::write(dir.join("config.toml"), contents).expect("config");
}

/// Graph with the inbox, two lists in folders, one list in "Archive" and
/// one in no folder; then a mock TypeSafe, and the daemon synced.
async fn setup(config: &str) -> (Env, FakeGraph, MockServer) {
    let mut env = Env::new();
    let typesafe = MockServer::start().await;
    env.typesafe_url = Some(typesafe.uri());
    write_config(&env, config);
    let graph = FakeGraph::start(
        &mut env,
        vec![
            list("L-tasks", "Tasks", "defaultList"),
            list("L-money", "Money", "none"),
            list("L-garden", "Garden", "none"),
            list("L-old", "Old launch", "none"),
            list("L-books", "Books", "none"),
        ],
    )
    .await;
    graph.edit(|data| {
        for (id, folder) in [
            ("L-money", "Areas"),
            ("L-garden", "Home"),
            ("L-old", "Archive"),
        ] {
            data.extensions.insert(
                id.into(),
                json!({ "extensionName": "com.planetaryescape.mstodo", "folder": folder }),
            );
        }
        data.tasks.insert(
            "L-money".into(),
            vec![
                task("T1", "Renew car insurance", "W/\"1\""),
                task("T2", "File expenses", "W/\"2\""),
            ],
        );
        data.tasks
            .insert("L-old".into(), vec![task("T3", "Ship it", "W/\"3\"")]);
    });
    env.synced();
    (env, graph, typesafe)
}

fn answer(choice: &str, confidence: f64) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "model": "jev-1.13.0",
        "answers": {
            "list": {
                "type": "choice",
                "choice": choice,
                "probabilities": { choice: confidence },
                "confidence": confidence
            }
        },
        "usage": { "input_tokens": 300, "output_tokens": 20 }
    }))
}

async fn answers(typesafe: &MockServer, choice: &str, confidence: f64) {
    Mock::given(method("POST"))
        .and(header("authorization", format!("Bearer {KEY}").as_str()))
        .respond_with(answer(choice, confidence))
        .mount(typesafe)
        .await;
}

async fn sent(typesafe: &MockServer) -> Vec<Value> {
    typesafe
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .map(|request: &Request| serde_json::from_slice(&request.body).expect("json body"))
        .collect()
}

#[tokio::test]
async fn suggest_list_names_a_likely_list_from_the_folders_names_and_tasks() {
    let (env, _graph, typesafe) = setup(ENABLED).await;
    answers(&typesafe, "Money", 0.91).await;

    let suggested = env.json(&["tasks", "suggest-list", "pay council tax"]);
    assert_eq!(suggested["schema_version"], 2);
    assert_eq!(suggested["title"], "pay council tax");
    assert_eq!(suggested["list_name"], "Money");
    assert_eq!(suggested["confidence"], 0.91);
    let money = env.local_id(&["lists", "list"], "L-money");
    assert_eq!(suggested["list_id"], money.as_str());

    let body = sent(&typesafe).await.remove(0);
    assert_eq!(body["model"], "jev-latest");
    assert_eq!(body["state"], json!({ "task_title": "pay council tax" }));
    let question = &body["questions"]["list"];
    assert_eq!(question["type"], "choice");
    // Newest first; not the inbox, and not the archived list.
    assert_eq!(
        question["criteria"],
        json!({
            "Money": "Areas list \"Money\". Example tasks: File expenses; Renew car insurance",
            "Garden": "Home list \"Garden\".",
            "Books": "List \"Books\"."
        })
    );

    // `ids` is the list's ID alone.
    let ids = env
        .cmd()
        .args([
            "--format",
            "ids",
            "tasks",
            "suggest-list",
            "pay council tax",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(String::from_utf8_lossy(&ids).trim(), money);
}

#[tokio::test]
async fn below_the_threshold_there_is_no_suggestion() {
    let (env, _graph, typesafe) = setup(ENABLED).await;
    answers(&typesafe, "Books", 0.55).await;
    let suggested = env.json(&["tasks", "suggest-list", "think about things"]);
    assert_eq!(suggested["list_id"], Value::Null);
    assert_eq!(suggested["list_name"], Value::Null);
    assert_eq!(suggested["confidence"], Value::Null);
    let table = env
        .cmd()
        .args(["--format", "table", "tasks", "suggest-list", "think"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(String::from_utf8_lossy(&table).contains("no suggestion"));
}

#[tokio::test]
async fn adding_to_the_inbox_notes_a_likely_list_without_moving_the_task() {
    let (env, graph, typesafe) = setup(ENABLED).await;
    answers(&typesafe, "Money", 0.86).await;
    Mock::given(method("POST"))
        .and(wiremock::matchers::path(
            "/v1.0/me/todo/lists/L-tasks/tasks",
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(task(
            "T9",
            "pay council tax",
            "W/\"9\"",
        )))
        .mount(&graph.server)
        .await;

    let output = env
        .cmd()
        .args(["--format", "table", "tasks", "add", "pay council tax"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("note: suggested list: Money (0.86)")
            && stderr.contains("tasks move")
            && stderr.contains("--to Money"),
        "{stderr}"
    );
    env.settled();
    let inbox = env.json(&["tasks", "list"]);
    assert_eq!(inbox["items"][0]["title"], "pay council tax", "{inbox}");

    // A dry run is told to add it there instead.
    let dry = env
        .cmd()
        .args(["--format", "table", "tasks", "add", "pay rent", "--dry-run"])
        .assert()
        .success()
        .get_output()
        .clone();
    assert!(
        String::from_utf8_lossy(&dry.stderr).contains("re-run with --list Money or #Money"),
        "{}",
        String::from_utf8_lossy(&dry.stderr)
    );

    // JSON's stderr is the output contract: no note, and nobody asked.
    let asked = sent(&typesafe).await.len();
    let json = env
        .cmd()
        .args(["--format", "json", "tasks", "add", "pay rent", "--dry-run"])
        .assert()
        .success()
        .get_output()
        .clone();
    assert!(json.stderr.is_empty());
    // A task with a list isn't headed for the inbox: nothing is asked.
    env.cmd()
        .args([
            "--format",
            "table",
            "tasks",
            "add",
            "pay rent #Books",
            "--dry-run",
        ])
        .assert()
        .success();
    assert_eq!(sent(&typesafe).await.len(), asked);
}

#[tokio::test]
async fn a_failing_provider_is_no_suggestion_and_doctor_says_why() {
    let (env, _graph, typesafe) = setup(ENABLED).await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&typesafe)
        .await;
    let suggested = env.json(&["tasks", "suggest-list", "pay council tax"]);
    assert_eq!(suggested["list_id"], Value::Null);

    let doctor = env.json(&["doctor"]);
    assert_eq!(doctor["suggest"]["enabled"], true);
    assert_eq!(doctor["suggest"]["provider"], "typesafe");
    assert!(
        doctor["suggest"]["sends"]
            .as_str()
            .is_some_and(|sends| sends.contains("task titles") && sends.contains("TypeSafe")),
        "{doctor}"
    );
    assert!(
        doctor["problems"]
            .as_array()
            .expect("problems")
            .iter()
            .any(|problem| problem == "suggest: TypeSafe answered 500"),
        "{doctor}"
    );
    // Warned once, however often it fails, and the key never logged.
    env.json(&["tasks", "suggest-list", "pay council tax"]);
    let database = doctor["database"]["path"].as_str().expect("database path");
    let log_file = std::path::Path::new(database)
        .parent()
        .expect("data dir")
        .join("logs/daemon.log");
    let log = std::fs::read_to_string(&log_file).expect("daemon log");
    assert_eq!(log.matches("no list suggestion").count(), 1, "{log}");
    assert!(!log.contains(KEY), "{log}");
}

#[tokio::test]
async fn off_by_default_and_saying_how_to_turn_it_on() {
    let (env, _graph, typesafe) = setup("[tui]\ntheme = \"default\"\n").await;
    let error = env.failure(&["tasks", "suggest-list", "pay council tax"], 2);
    assert_eq!(error["error"]["kind"], "invalid_input", "{error}");
    assert!(
        error["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("[suggest]")),
        "{error}"
    );
    // `tasks add` is unaffected, and nothing was sent.
    env.cmd()
        .args(["--format", "table", "tasks", "add", "pay rent", "--dry-run"])
        .assert()
        .success();
    assert!(sent(&typesafe).await.is_empty());
    let doctor = env.json(&["doctor"]);
    assert_eq!(doctor["suggest"]["enabled"], false);
    assert_eq!(doctor["suggest"]["sends"], Value::Null);
}

#[tokio::test]
async fn a_slow_suggestion_never_holds_up_the_connections_other_requests() {
    let (env, _graph, typesafe) = setup(ENABLED).await;
    Mock::given(method("POST"))
        .respond_with(answer("Money", 0.9).set_delay(Duration::from_millis(1500)))
        .mount(&typesafe)
        .await;
    let stream = UnixStream::connect(env.socket()).await.expect("connect");
    let mut framed = Framed::new(stream, Codec::new());
    for (id, request) in [
        (
            1,
            IpcRequest::SuggestList {
                title: "pay council tax".into(),
            },
        ),
        (2, IpcRequest::Status),
    ] {
        framed
            .send(Message {
                id,
                payload: Payload::Request(request),
            })
            .await
            .expect("send");
    }
    let mut order = Vec::new();
    while order.len() < 2 {
        let message = tokio::time::timeout(Duration::from_secs(10), framed.next())
            .await
            .expect("an answer within 10 seconds")
            .expect("open")
            .expect("readable");
        if let Payload::Response(Response::Ok { data }) = message.payload {
            order.push(message.id);
            if message.id == 1 {
                assert!(
                    matches!(data, ResponseData::ListSuggestion { suggestion: Some(ref list) } if list.list_name == "Money"),
                    "{data:?}"
                );
            }
        }
    }
    assert_eq!(order, [2, 1], "Status waited for the suggestion");
}

/// What the daemon logged, from `doctor`'s database path.
fn daemon_log(env: &Env) -> String {
    let doctor = env.json(&["doctor"]);
    let database = doctor["database"]["path"].as_str().expect("database path");
    let log_file = std::path::Path::new(database)
        .parent()
        .expect("data dir")
        .join("logs/daemon.log");
    std::fs::read_to_string(log_file).expect("daemon log")
}

#[tokio::test]
async fn a_failing_key_command_shows_its_status_never_its_output() {
    let (env, _graph, typesafe) = setup(
        r#"
[suggest]
enabled = true
api_key_command = ["sh", "-c", "echo sk-fake-stdout; echo sk-fake-secret-on-stderr >&2; exit 3"]
"#,
    )
    .await;
    let output = env
        .cmd()
        .args([
            "--format",
            "json",
            "tasks",
            "suggest-list",
            "pay council tax",
        ])
        .assert()
        .success()
        .get_output()
        .clone();
    assert!(sent(&typesafe).await.is_empty(), "no key, nothing sent");
    let doctor = env.json(&["doctor"]);
    let problem = doctor["suggest"]["problem"].as_str().unwrap_or_default();
    assert!(
        problem.contains("exit status: 3") && problem.contains("output withheld"),
        "{doctor}"
    );
    for seen in [
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        doctor.to_string(),
        daemon_log(&env),
    ] {
        assert!(!seen.contains("sk-fake"), "{seen}");
    }
}

#[tokio::test]
async fn an_unknown_choice_is_reported_without_echoing_it() {
    let (env, _graph, typesafe) = setup(ENABLED).await;
    // A model answer that repeats the (private) title as its choice.
    answers(&typesafe, "private title 7c1f", 0.95).await;
    let output = env
        .cmd()
        .args([
            "--format",
            "json",
            "tasks",
            "suggest-list",
            "private title 7c1f",
        ])
        .assert()
        .success()
        .get_output()
        .clone();
    let suggested: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(suggested["list_id"], Value::Null);
    let doctor = env.json(&["doctor"]);
    assert_eq!(
        doctor["suggest"]["problem"], "the model returned an unknown list",
        "{doctor}"
    );
    let log = daemon_log(&env);
    assert!(log.contains("the model returned an unknown list"), "{log}");
    for seen in [
        String::from_utf8_lossy(&output.stderr).into_owned(),
        doctor.to_string(),
        log,
    ] {
        assert!(!seen.contains("7c1f"), "{seen}");
    }
}
