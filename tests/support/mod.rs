#![allow(dead_code, reason = "each test binary uses a different subset")]

//! The test environment for driving the real binary and the daemon it
//! starts, with HOME, the XDG directories and the runtime directory in a
//! temp dir, so nothing touches the developer's own sign-in or daemon. Every
//! test stops its daemon, even when it fails.

pub mod fake_graph;
pub mod fake_moves;

use std::path::PathBuf;

use assert_cmd::Command;
use serde_json::{Value, json};

pub const ACCESS_TOKEN: &str = "test-access-token";

pub struct Env {
    pub home: tempfile::TempDir,
    pub graph_url: Option<String>,
    /// A mock TypeSafe for list suggestions, set before the daemon starts.
    pub typesafe_url: Option<String>,
}

impl Env {
    pub fn new() -> Self {
        // Under /tmp, not the platform temp dir: macOS caps a socket path at
        // 104 bytes, and $TMPDIR plus "Library/Application Support/…" is over.
        let home = tempfile::Builder::new()
            .prefix("mt")
            .tempdir_in("/tmp")
            .expect("tempdir");
        Self {
            home,
            graph_url: None,
            typesafe_url: None,
        }
    }

    pub fn cmd(&self) -> Command {
        self.cmd_at(&assert_cmd::cargo::cargo_bin("ms-todo"))
    }

    /// Like [`Env::cmd`], but runs `program`, such as a symlink to the binary.
    pub fn cmd_at(&self, program: &std::path::Path) -> Command {
        let home = self.home.path();
        let mut command = Command::new(program);
        command
            .env("HOME", home)
            .env("XDG_DATA_HOME", home.join("data"))
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("XDG_RUNTIME_DIR", home.join("run"))
            .env_remove("MS_TODO_INSTANCE")
            .env_remove("MS_TODO_CONFIG_DIR")
            .env_remove("MS_TODO_GRAPH_URL")
            // List suggestions reach TypeSafe only when a test says so.
            .env_remove("MS_TODO_TYPESAFE_URL")
            .env_remove("TYPESAFE_API_KEY")
            // Due dates and reminders are written in this zone.
            .env("TZ", "Europe/London");
        if let Some(url) = &self.graph_url {
            command.env("MS_TODO_GRAPH_URL", url);
        }
        if let Some(url) = &self.typesafe_url {
            command.env("MS_TODO_TYPESAFE_URL", url);
        }
        command
    }

    pub fn json(&self, args: &[&str]) -> Value {
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

    /// Run `args` with `--format json`, expect exit `code`, and return the
    /// JSON error from stderr.
    pub fn failure(&self, args: &[&str], code: i32) -> Value {
        let output = self
            .cmd()
            .args(["--format", "json"])
            .args(args)
            .assert()
            .code(code)
            .get_output()
            .clone();
        serde_json::from_slice(&output.stderr).expect("json error on stderr")
    }

    /// Run a sync and wait for it, so reads answer from a filled cache.
    pub fn synced(&self) -> Value {
        self.json(&["sync", "--wait"])
    }

    /// The local ID of the list or task whose Graph ID is `graph_id`, in
    /// the items of `collection` (`lists list`, or `tasks list --list L`).
    pub fn local_id(&self, collection: &[&str], graph_id: &str) -> String {
        let items = self.json(collection)["items"].clone();
        let found = items
            .as_array()
            .and_then(|items| items.iter().find(|item| item["graph_id"] == graph_id))
            .and_then(|item| item["id"].as_str());
        assert!(found.is_some(), "no {graph_id} in {collection:?}: {items}");
        found.expect("checked above").to_owned()
    }

    /// Wait up to 10 seconds for the running sync to finish.
    pub fn wait_until_idle(&self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while self.json(&["doctor"])["syncing"] != false {
            assert!(
                std::time::Instant::now() < deadline,
                "the sync never finished"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    pub fn socket(&self) -> PathBuf {
        PathBuf::from(
            self.json(&["daemon", "status"])["socket"]
                .as_str()
                .expect("socket"),
        )
    }

    /// A signed-in credential good for an hour, so the daemon never refreshes.
    pub fn sign_in(&self) {
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

    /// Every outbox operation, newest first.
    pub fn outbox(&self) -> Vec<Value> {
        self.json(&["outbox", "list"])["items"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    /// Wait up to 20 seconds until no write is pending or being sent.
    pub fn settled(&self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            let outbox = self.outbox();
            if outbox
                .iter()
                .all(|op| op["state"] != "pending" && op["state"] != "inflight")
            {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the outbox never settled: {outbox:?}"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    /// Wait up to 20 seconds until operation `op_id` is in `state`, and
    /// return it.
    pub fn op_in_state(&self, op_id: &str, state: &str) -> Value {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            let op = self
                .outbox()
                .into_iter()
                .find(|op| op["op_id"] == op_id)
                .unwrap_or(Value::Null);
            if op["state"] == state {
                return op;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "operation {op_id} never became {state}: {op}"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = self.cmd().args(["daemon", "stop"]).output();
    }
}
