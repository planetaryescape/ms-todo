// Adapted from spotuify crates/spotuify-spotify/src/auth.rs
// (`TOKEN_LOCK_TIMEOUT`, `acquire_token_store_lock_with_timeout`, `read_token_file`)
// @ d807e5e4f9d2f09878cdc22309af3589623f7785.

//! `<data_dir>/ms-todo[-<instance>]/auth/token.json`, mode 0600 in a 0700
//! directory, with every write behind an exclusive `fs2` lock on
//! `auth/token.lock` (docs/blueprint/03-graph-provider.md#sign-in).

use std::fs::{File, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fs2::FileExt;
use serde::{Deserialize, Serialize};

use super::AuthError;
use super::private_file::{atomic_write_mode_0600, ensure_private_dir};

/// Refresh this long before the access token expires (mxr's `REFRESH_MARGIN_SECS`).
pub(crate) const REFRESH_MARGIN_SECS: i64 = 300;
const TOKEN_LOCK_TIMEOUT: Duration = Duration::from_secs(15);
const TOKEN_LOCK_POLL: Duration = Duration::from_millis(50);

/// The whole credential. It's always saved as a unit, so a refresh can never
/// leave a new access token next to an old refresh token.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredToken {
    pub access_token: String,
    pub refresh_token: String,
    /// Unix seconds when the access token expires.
    pub expires_at: i64,
    /// Scopes the token response granted. Microsoft leaves `offline_access`
    /// out of this list even though it issues a refresh token (spike S5).
    #[serde(default)]
    pub scopes: Vec<String>,
    /// The Entra app the refresh token belongs to. A refresh must use the
    /// same client ID, even if the configured one has changed since.
    pub client_id: String,
}

impl StoredToken {
    /// Seconds until the access token expires; negative once it has.
    pub fn expires_in_secs(&self) -> i64 {
        self.expires_at - chrono::Utc::now().timestamp()
    }

    pub fn is_near_expiry(&self) -> bool {
        self.expires_in_secs() < REFRESH_MARGIN_SECS
    }
}

#[derive(Clone, Debug)]
pub(crate) struct TokenStore {
    dir: PathBuf,
}

/// Holds the exclusive lock until dropped. Closing the file releases the
/// `flock`, so there's no explicit unlock to forget.
pub(crate) struct TokenLock {
    _file: File,
}

impl TokenStore {
    pub(crate) fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub(crate) fn token_path(&self) -> PathBuf {
        self.dir.join("token.json")
    }

    fn lock_path(&self) -> PathBuf {
        self.dir.join("token.lock")
    }

    /// Take the lock without blocking the async runtime.
    pub(crate) async fn lock(&self) -> Result<TokenLock, AuthError> {
        let dir = self.dir.clone();
        let lock_path = self.lock_path();
        tokio::task::spawn_blocking(move || acquire_lock(&dir, &lock_path, TOKEN_LOCK_TIMEOUT))
            .await
            .map_err(|join| AuthError::store(self.lock_path(), std::io::Error::other(join)))?
    }

    pub(crate) fn load(&self) -> Result<Option<StoredToken>, AuthError> {
        let path = self.token_path();
        let raw = match std::fs::read(&path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(AuthError::store(path, error)),
        };
        serde_json::from_slice(&raw)
            .map(Some)
            .map_err(|error| AuthError::store(path, std::io::Error::other(error)))
    }

    /// The `&TokenLock` argument makes "save without the lock" a compile error.
    pub(crate) fn save(&self, _lock: &TokenLock, token: &StoredToken) -> Result<(), AuthError> {
        let path = self.token_path();
        let raw = serde_json::to_vec_pretty(token)
            .map_err(|error| AuthError::store(&path, std::io::Error::other(error)))?;
        atomic_write_mode_0600(&path, &raw).map_err(|error| AuthError::store(path, error))
    }

    /// Returns whether there was a token to delete.
    pub(crate) fn delete(&self, _lock: &TokenLock) -> Result<bool, AuthError> {
        let path = self.token_path();
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
            Err(error) => Err(AuthError::store(path, error)),
        }
    }
}

fn acquire_lock(dir: &Path, lock_path: &Path, timeout: Duration) -> Result<TokenLock, AuthError> {
    ensure_private_dir(dir).map_err(|error| AuthError::store(dir, error))?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
        .map_err(|error| AuthError::store(lock_path, error))?;
    let started = Instant::now();
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(TokenLock { _file: file }),
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                if started.elapsed() >= timeout {
                    return Err(AuthError::LockTimeout(lock_path.to_path_buf()));
                }
                let remaining = timeout.saturating_sub(started.elapsed());
                std::thread::sleep(TOKEN_LOCK_POLL.min(remaining));
            }
            Err(error) => return Err(AuthError::store(lock_path, error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_lock_times_out_other_takers() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("auth");
        let lock_path = dir.join("token.lock");
        let _held = acquire_lock(&dir, &lock_path, TOKEN_LOCK_TIMEOUT).expect("first lock");

        let second = acquire_lock(&dir, &lock_path, Duration::from_millis(100));
        assert!(matches!(second, Err(AuthError::LockTimeout(_))));
    }
}
