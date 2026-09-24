//! Drives the real binary and the daemon it starts, with HOME, the XDG
//! directories and the runtime directory in a temp dir, so nothing touches
//! the developer's own sign-in or daemon. Every test stops its daemon, even
//! when it fails.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ACCESS_TOKEN: &str = "test-access-token";

struct Env {
    home: tempfile::TempDir,
    graph_url: Option<String>,
}

impl Env {
    fn new() -> Self {
        // Under /tmp, not the platform temp dir: macOS caps a socket path at
        // 104 bytes, and $TMPDIR plus "Library/Application Support/…" is over.
        let home = tempfile::Builder::new()
            .prefix("mt")
            .tempdir_in("/tmp")
            .expect("tempdir");
        Self {
            home,
            graph_url: None,
        }
    }

    fn cmd(&self) -> Command {
        let home = self.home.path();
        let mut command = Command::cargo_bin("ms-todo").expect("ms-todo binary");
        command
            .env("HOME", home)
            .env("XDG_DATA_HOME", home.join("data"))
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("XDG_RUNTIME_DIR", home.join("run"))
            .env_remove("MS_TODO_INSTANCE")
            .env_remove("MS_TODO_CONFIG_DIR")
            .env_remove("MS_TODO_GRAPH_URL");
        if let Some(url) = &self.graph_url {
            command.env("MS_TODO_GRAPH_URL", url);
        }
        command
    }

    fn json(&self, args: &[&str]) -> Value {
        let output = self
            .cmd()
            .args(["--format", "json"])
            .args(args)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        serde_json::from_slice(&output).expect("json output")
    }

    fn socket(&self) -> PathBuf {
        PathBuf::from(
            self.json(&["daemon", "status"])["socket"]
                .as_str()
                .expect("socket"),
        )
    }

    /// A signed-in credential good for an hour, so the daemon never refreshes.
    fn sign_in(&self) {
        let logout = self.json(&["auth", "logout"]);
        let token_path = PathBuf::from(logout["token_path"].as_str().expect("token path"));
        std::fs::create_dir_all(token_path.parent().expect("auth dir")).expect("mkdir");
        let expires_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_secs()
            + 3600;
        let token = json!({
            "access_token": ACCESS_TOKEN,
            "refresh_token": "test-refresh-token",
            "expires_at": expires_at,
            "scopes": ["Tasks.ReadWrite"],
            "client_id": "test-client"
        });
        std::fs::write(token_path, token.to_string()).expect("write token");
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = self.cmd().args(["daemon", "stop"]).output();
    }
}

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
    assert_eq!(started["protocol_version"], 1);
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

async fn graph_with_lists(env: &mut Env) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1.0/me/todo/lists"))
        .and(header("Authorization", format!("Bearer {ACCESS_TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "value": [
            { "id": "L-tasks", "displayName": "Tasks", "wellknownListName": "defaultList", "isShared": false },
            { "id": "L-a", "displayName": "Groceries", "wellknownListName": "none", "isShared": false },
            { "id": "L-b", "displayName": "Groceries", "wellknownListName": "none", "isShared": true }
        ]})))
        .mount(&server)
        .await;
    env.graph_url = Some(format!("{}/v1.0", server.uri()));
    env.sign_in();
    server
}

#[tokio::test]
async fn an_ambiguous_list_name_exits_2_with_the_candidates() {
    let mut env = Env::new();
    let _graph = graph_with_lists(&mut env).await;

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
    assert_eq!(ids, ["L-a", "L-b"]);
}

#[tokio::test]
async fn lists_and_tasks_come_back_in_the_collection_envelope() {
    let mut env = Env::new();
    let graph = graph_with_lists(&mut env).await;
    Mock::given(method("GET"))
        .and(path("/v1.0/me/todo/lists/L-tasks/tasks"))
        .and(header("Prefer", "odata.maxpagesize=200"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "value": [
            { "id": "T1", "title": "Buy milk", "status": "notStarted", "importance": "high" }
        ]})))
        .mount(&graph)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1.0/me/todo/lists/L-b/tasks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "value": [] })))
        .mount(&graph)
        .await;

    let lists = env.json(&["lists", "list"]);
    assert_eq!(lists["schema_version"], 1);
    assert_eq!(lists["items"].as_array().expect("items").len(), 3);
    assert_eq!(lists["items"][1]["displayName"], "Groceries");

    // No --list: the default "Tasks" list, with every Graph field kept.
    let tasks = env.json(&["tasks", "list"]);
    assert_eq!(tasks["schema_version"], 1);
    assert_eq!(tasks["items"][0]["id"], "T1");
    assert_eq!(tasks["items"][0]["importance"], "high");

    // An ID picks one of the two same-named lists.
    let by_id = env.json(&["tasks", "list", "--list", "L-b"]);
    assert_eq!(by_id["items"], json!([]));

    let ids = env
        .cmd()
        .args(["--format", "ids", "lists", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(String::from_utf8(ids).expect("utf8"), "L-tasks\nL-a\nL-b\n");

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
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "displayName": "BK" })))
        .mount(&graph)
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
