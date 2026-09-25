//! The daemon's lifecycle and reads, through the real binary; see
//! `support` for the environment.

mod support;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use nix::unistd::{Pid, getpgid, getsid};
use serde_json::{Value, json};
use support::fake_graph::{FakeGraph, list, task};
use support::{ACCESS_TOKEN, Env};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, ResponseTemplate};

fn pid_exists(pid: u64) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .expect("run kill")
        .success()
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).expect("stat").permissions().mode() & 0o777
}

#[test]
fn start_status_stop_and_the_pid_is_gone() {
    let env = Env::new();

    let started = env.json(&["daemon", "start"]);
    assert_eq!(started["running"], true);
    assert_eq!(started["ready"], true);
    assert_eq!(started["protocol_version"], 6);
    assert_eq!(started["instance"], "dev");
    let pid = started["pid"].as_u64().expect("pid");
    assert!(pid_exists(pid));

    let status = env.json(&["daemon", "status"]);
    assert_eq!(status["pid"], pid);
    let socket = env.socket();
    assert_eq!(mode(&socket), 0o600, "socket mode");
    assert_eq!(
        mode(socket.parent().expect("run dir")),
        0o700,
        "run dir mode"
    );

    // The cache holds tasks: private like the socket.
    let database = std::path::PathBuf::from(
        env.json(&["doctor"])["database"]["path"]
            .as_str()
            .expect("database path"),
    );
    assert_eq!(mode(&database), 0o600, "database mode");
    assert_eq!(
        mode(database.parent().expect("data dir")),
        0o700,
        "data dir mode"
    );

    // A second start finds the running daemon rather than starting another.
    assert_eq!(env.json(&["daemon", "start"])["pid"], pid);

    let stopped = env.json(&["daemon", "stop"]);
    assert_eq!(stopped["stopped_pid"], pid);
    assert!(!pid_exists(pid), "daemon pid {pid} still exists after stop");
    assert!(std::os::unix::net::UnixStream::connect(&socket).is_err());
    assert_eq!(env.json(&["daemon", "status"])["running"], false);

    // Stopping again is a no-op.
    assert_eq!(env.json(&["daemon", "stop"])["stopped_pid"], Value::Null);
}

/// An auto-started daemon must never stay a client's child: a client that
/// never reaps it (the TUI) would leave a zombie that blocks `daemon stop`
/// (D-046). Its own session also shows it left the client's.
#[test]
fn auto_start_detaches_the_daemon_from_its_client() {
    let env = Env::new();

    let client = env
        .cmd()
        .args(["--format", "json", "daemon", "start"])
        .output()
        .expect("run daemon start");
    assert!(client.status.success(), "{client:?}");
    let started: Value = serde_json::from_slice(&client.stdout).expect("json");
    let pid = started["pid"].as_u64().expect("pid");
    let daemon = Pid::from_raw(i32::try_from(pid).expect("pid fits i32"));

    let parent = std::process::Command::new("ps")
        .args(["-o", "ppid=", "-p", &pid.to_string()])
        .output()
        .expect("run ps");
    let parent: u32 = String::from_utf8_lossy(&parent.stdout)
        .trim()
        .parse()
        .expect("ppid");
    assert_ne!(
        parent,
        std::process::id(),
        "the test harness parents the daemon"
    );

    let ours = getsid(None).expect("our session");
    let its = getsid(Some(daemon)).expect("the daemon's session");
    assert_ne!(its, ours, "the daemon stayed in the client's session");
    assert_ne!(
        its, daemon,
        "the daemon leads a session, so could take a terminal"
    );
    assert!(
        getpgid(Some(daemon)).is_ok_and(|group| group != getpgid(None).expect("our group")),
        "the daemon stayed in the client's process group"
    );
}

#[test]
fn a_stale_socket_is_recovered_by_auto_start_and_signed_out_exits_4() {
    let env = Env::new();
    let socket = env.socket();
    std::fs::create_dir_all(socket.parent().expect("run dir")).expect("mkdir");
    // A socket file nobody listens on, as a crashed daemon leaves behind.
    drop(std::os::unix::net::UnixListener::bind(&socket).expect("bind"));
    assert!(socket.exists());

    let assert = env
        .cmd()
        .args(["--format", "json", "lists", "list"])
        .assert()
        .code(4);

    let output = assert.get_output();
    assert!(output.stdout.is_empty(), "errors never go to stdout");
    let error: Value = serde_json::from_slice(&output.stderr).expect("json error");
    assert_eq!(error["error"]["kind"], "auth_required");
    let message = error["error"]["message"].as_str().expect("message");
    assert!(message.contains("ms-todo auth login"), "{message}");
    let status = env.json(&["daemon", "status"]);
    assert_eq!(status["ready"], true, "auto-start left no ready daemon");
    assert_eq!(status["signed_in"], false);
}

#[test]
fn auth_bearer_without_reveal_secret_exits_2_before_starting_a_daemon() {
    let env = Env::new();

    let assert = env
        .cmd()
        .args(["--format", "json", "auth", "bearer"])
        .assert()
        .code(2);

    let error: Value = serde_json::from_slice(&assert.get_output().stderr).expect("json error");
    assert_eq!(error["error"]["kind"], "invalid_input");
    assert!(
        error["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("--reveal-secret"))
    );
    assert_eq!(env.json(&["daemon", "status"])["running"], false);
}

/// Graph with "Tasks" and two lists both named "Groceries", signed in.
async fn graph_with_lists(env: &mut Env) -> FakeGraph {
    let graph = FakeGraph::start(
        env,
        vec![
            list("L-tasks", "Tasks", "defaultList"),
            list("L-a", "Groceries", "none"),
            list("L-b", "Groceries", "none"),
        ],
    )
    .await;
    graph.edit(|data| {
        let mut milk = task("T1", "Buy milk", "W/\"e1\"");
        milk["importance"] = json!("high");
        data.tasks.insert("L-tasks".into(), vec![milk]);
        data.tasks.insert("L-a".into(), Vec::new());
        data.tasks.insert("L-b".into(), Vec::new());
    });
    graph
}

#[tokio::test]
async fn an_ambiguous_list_name_exits_2_with_the_candidates() {
    let mut env = Env::new();
    let _graph = graph_with_lists(&mut env).await;
    env.synced();

    let assert = env
        .cmd()
        .args(["--format", "json", "tasks", "list", "--list", "Groceries"])
        .assert()
        .code(2);

    let error: Value = serde_json::from_slice(&assert.get_output().stderr).expect("json error");
    assert_eq!(error["error"]["kind"], "invalid_input");
    let ids: Vec<&str> = error["error"]["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .filter_map(|candidate| candidate["id"].as_str())
        .collect();
    let lists = ["lists", "list"];
    assert_eq!(
        ids,
        [env.local_id(&lists, "L-a"), env.local_id(&lists, "L-b")]
    );
}

#[tokio::test]
async fn lists_and_tasks_come_back_in_the_collection_envelope() {
    let mut env = Env::new();
    let graph = graph_with_lists(&mut env).await;
    env.synced();

    let lists = env.json(&["lists", "list"]);
    assert_eq!(lists["schema_version"], 2);
    assert_eq!(lists["sync"]["state"], "ready");
    assert_eq!(lists["items"].as_array().expect("items").len(), 3);
    assert_eq!(lists["items"][1]["displayName"], "Groceries");
    assert_eq!(lists["items"][1]["graph_id"], "L-a");
    assert_eq!(lists["items"][1]["sync_state"], "synced");

    // No --list: the default "Tasks" list, with every Graph field kept.
    let tasks = env.json(&["tasks", "list"]);
    assert_eq!(tasks["schema_version"], 2);
    assert_eq!(tasks["sync"]["state"], "ready");
    let milk = &tasks["items"][0];
    assert_eq!(milk["graph_id"], "T1");
    assert_eq!(milk["importance"], "high");
    assert_eq!(milk["list_id"], lists["items"][0]["id"]);
    let local = milk["id"].as_str().expect("local id");
    assert_eq!(local.len(), 36, "a UUID: {local}");

    // Either ID picks one of the two same-named lists.
    let by_graph_id = env.json(&["tasks", "list", "--list", "L-b"]);
    assert_eq!(by_graph_id["items"], json!([]));
    let local_b = lists["items"][2]["id"].as_str().expect("id");
    assert_eq!(
        env.json(&["tasks", "list", "--list", local_b])["items"],
        json!([])
    );

    // Reads come from the cache: no request after the sync.
    let requests = graph
        .server
        .received_requests()
        .await
        .expect("recorded")
        .len();
    env.json(&["tasks", "list"]);
    assert_eq!(
        graph
            .server
            .received_requests()
            .await
            .expect("recorded")
            .len(),
        requests
    );

    let jsonl = env
        .cmd()
        .args(["--format", "jsonl", "lists", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let records: Vec<Value> = String::from_utf8(jsonl)
        .expect("utf8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("one JSON object per line"))
        .collect();
    assert_eq!(records.len(), 3);
    assert!(records.iter().all(|record| record["schema_version"] == 2));
    assert_eq!(records[2]["graph_id"], "L-b");

    let ids = env
        .cmd()
        .args(["--format", "ids", "lists", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let local_ids: Vec<&str> = lists["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter_map(|list| list["id"].as_str())
        .collect();
    assert_eq!(
        String::from_utf8(ids).expect("utf8"),
        format!("{}\n", local_ids.join("\n"))
    );

    let missing = env
        .cmd()
        .args(["--format", "json", "tasks", "list", "--list", "Nope"])
        .assert()
        .code(3);
    let error: Value = serde_json::from_slice(&missing.get_output().stderr).expect("json error");
    assert_eq!(error["error"]["kind"], "not_found");
}

#[tokio::test]
async fn raw_get_and_bearer_go_through_the_daemon() {
    let mut env = Env::new();
    let graph = graph_with_lists(&mut env).await;
    Mock::given(method("GET"))
        .and(path("/v1.0/me"))
        .and(header(
            "Authorization",
            format!("Bearer {ACCESS_TOKEN}").as_str(),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "displayName": "BK" })))
        .mount(&graph.server)
        .await;

    let me = env.json(&["raw", "GET", "/me"]);
    assert_eq!(me, json!({ "displayName": "BK" }), "raw has no envelope");

    let token = env
        .cmd()
        .args(["--format", "table", "auth", "bearer", "--reveal-secret"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        String::from_utf8(token).expect("utf8"),
        format!("{ACCESS_TOKEN}\n")
    );

    env.cmd()
        .args(["raw", "GET", "https://attacker.example/me"])
        .assert()
        .code(2);
}
