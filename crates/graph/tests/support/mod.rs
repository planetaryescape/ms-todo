#![allow(dead_code, reason = "each test binary uses a different subset")]

use std::path::{Path, PathBuf};
use std::time::Duration;

use ms_todo_graph::auth::{Authenticator, Endpoints, StoredToken};
use wiremock::MockServer;

pub const CLIENT_ID: &str = "test-client";

pub struct Fixture {
    pub server: MockServer,
    pub dir: tempfile::TempDir,
    pub auth: Authenticator,
}

impl Fixture {
    pub async fn new() -> Self {
        let server = MockServer::start().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let endpoints = Endpoints {
            authority: format!("{}/common/oauth2/v2.0", server.uri()),
            graph: format!("{}/v1.0", server.uri()),
        };
        let auth = Authenticator::new(dir.path().join("auth"), endpoints)
            .expect("authenticator")
            .with_poll_unit(Duration::from_millis(2));
        Self { server, dir, auth }
    }

    pub fn token_path(&self) -> PathBuf {
        self.auth.token_path()
    }

    pub fn write_token(&self, token: &StoredToken) {
        write_token_file(&self.token_path(), token);
    }

    pub fn read_token(&self) -> Option<StoredToken> {
        self.auth.stored_token().expect("read token")
    }
}

/// Writes straight to disk, bypassing the store and its lock, the way a
/// careless writer or a second machine would.
pub fn write_token_file(path: &Path, token: &StoredToken) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, serde_json::to_vec(token).expect("encode")).expect("write");
}

pub fn token(refresh_token: &str, expires_in: i64) -> StoredToken {
    StoredToken {
        access_token: format!("access-for-{refresh_token}"),
        refresh_token: refresh_token.into(),
        expires_at: now() + expires_in,
        scopes: vec!["Tasks.ReadWrite".into()],
        client_id: CLIENT_ID.into(),
    }
}

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}
