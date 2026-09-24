//! Sign-in: the public auth API. The CLI may use this module and nothing
//! else in the crate (`tests/workspace_boundaries.rs`). From rung 1 the
//! daemon is the only caller of `valid_token` and the refresh methods; the
//! CLI keeps `auth login|status|logout`, which work without a daemon (D-031).

mod account;
mod client_id;
mod device_code;
mod error;
mod refresh;
mod token_response;
mod token_store;

use std::path::PathBuf;
use std::time::Duration;

pub use account::Account;
pub use client_id::{ClientId, ClientIdSource, resolve_client_id};
pub use device_code::DeviceCode;
pub use error::AuthError;
pub use token_store::StoredToken;

use token_store::TokenStore;

/// Delegated scopes ms-todo asks for (docs/setup/entra-app-registration.md).
/// How to register your own Entra app (D-025).
pub const SETUP_GUIDE_URL: &str =
    "https://github.com/planetaryescape/ms-todo/blob/main/docs/setup/entra-app-registration.md";

pub const SCOPES: &str = "offline_access Tasks.ReadWrite MailboxSettings.ReadWrite User.Read";

/// Upper bound on every sign-in HTTP call. reqwest has no default timeout,
/// so a half-open socket would otherwise hang the caller; mxr's Gmail
/// provider was bitten by exactly this.
const AUTH_HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const AUTH_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Where to reach Microsoft. Tests point these at a mock server.
#[derive(Clone, Debug)]
pub struct Endpoints {
    /// OAuth 2.0 base, without the trailing `/devicecode` or `/token`.
    pub authority: String,
    /// Graph base URL.
    pub graph: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            authority: "https://login.microsoftonline.com/common/oauth2/v2.0".into(),
            graph: "https://graph.microsoft.com/v1.0".into(),
        }
    }
}

pub struct Authenticator {
    http: reqwest::Client,
    endpoints: Endpoints,
    store: TokenStore,
    poll_unit: Duration,
}

impl Authenticator {
    /// `auth_dir` is `<data_dir>/ms-todo[-<instance>]/auth`.
    pub fn new(auth_dir: PathBuf, endpoints: Endpoints) -> Result<Self, AuthError> {
        let http = reqwest::Client::builder()
            .timeout(AUTH_HTTP_TIMEOUT)
            .connect_timeout(AUTH_CONNECT_TIMEOUT)
            .user_agent(concat!("ms-todo/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            http,
            endpoints,
            store: TokenStore::new(auth_dir),
            poll_unit: Duration::from_secs(1),
        })
    }

    /// Scale the device-code poll interval. Only tests change this, so they
    /// can exercise `slow_down` without waiting real seconds.
    #[doc(hidden)]
    pub fn with_poll_unit(mut self, unit: Duration) -> Self {
        self.poll_unit = unit;
        self
    }

    pub fn token_path(&self) -> PathBuf {
        self.store.token_path()
    }

    /// The stored credential as it is on disk, without refreshing.
    pub fn stored_token(&self) -> Result<Option<StoredToken>, AuthError> {
        self.store.load()
    }

    /// Step 1 of `auth login`: get a code for the user to enter.
    pub async fn start_device_flow(&self, client_id: &str) -> Result<DeviceCode, AuthError> {
        device_code::start(&self.http, &self.endpoints.authority, client_id).await
    }

    /// Step 2 of `auth login`: wait for the user, then save the credential.
    pub async fn finish_device_flow(
        &self,
        client_id: &str,
        code: &DeviceCode,
    ) -> Result<StoredToken, AuthError> {
        let token = device_code::poll(
            &self.http,
            &self.endpoints.authority,
            client_id,
            code,
            self.poll_unit,
        )
        .await?;
        let lock = self.store.lock().await?;
        self.store.save(&lock, &token)?;
        Ok(token)
    }

    /// A token that is good for at least `REFRESH_MARGIN_SECS`, refreshing
    /// it first if needed. Never starts an interactive sign-in.
    pub async fn valid_token(&self) -> Result<StoredToken, AuthError> {
        let token = self.store.load()?.ok_or(AuthError::NotSignedIn)?;
        if !token.is_near_expiry() {
            return Ok(token);
        }
        self.refresh(&token).await
    }

    /// Refresh `stale`, unless the stored credential has already moved on.
    /// The HTTP client also calls this when Graph answers 401.
    pub async fn refresh(&self, stale: &StoredToken) -> Result<StoredToken, AuthError> {
        refresh::refresh_compare_and_swap(&self.http, &self.endpoints.authority, &self.store, stale)
            .await
    }

    pub async fn account(&self, token: &StoredToken) -> Result<Account, AuthError> {
        account::fetch_me(&self.http, &self.endpoints.graph, &token.access_token).await
    }

    /// Delete the stored credential. Returns whether there was one.
    pub async fn sign_out(&self) -> Result<bool, AuthError> {
        let lock = self.store.lock().await?;
        self.store.delete(&lock)
    }
}
