//! `ms-todo auth login|status|logout`. These run without a daemon (D-033
//! item 4). From rung 1 the daemon owns refresh, and `status` will ask it.

use ms_todo_core::Paths;
use ms_todo_graph::auth::{
    Account, Authenticator, ClientId, ClientIdSource, SETUP_GUIDE_URL, StoredToken,
    resolve_client_id,
};
use serde::Serialize;

use crate::error::CliError;
use crate::output::Render;

#[derive(Serialize)]
pub struct AuthStatus {
    pub signed_in: bool,
    pub account: Option<String>,
    pub display_name: Option<String>,
    /// RFC 3339, UTC.
    pub expires_at: String,
    pub expires_in_seconds: i64,
    /// The configured client ID, or null if none is configured.
    pub client_id: Option<String>,
    pub client_id_source: Option<&'static str>,
    /// Present only when the stored sign-in belongs to a different client ID
    /// than the configured one; refresh keeps using the token's own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_client_id: Option<String>,
    pub scopes: Vec<String>,
    pub token_path: String,
    pub instance: String,
}

impl AuthStatus {
    fn new(
        paths: &Paths,
        auth: &Authenticator,
        token: &StoredToken,
        account: &Account,
        configured: Option<ClientId>,
    ) -> Self {
        let token_client_id = match &configured {
            Some(configured) if configured.value == token.client_id => None,
            _ => Some(token.client_id.clone()),
        };
        Self {
            signed_in: true,
            account: account.sign_in_name().map(str::to_owned),
            display_name: account.display_name.clone(),
            expires_at: rfc3339(token.expires_at),
            expires_in_seconds: token.expires_in_secs(),
            client_id: configured.as_ref().map(|id| id.value.clone()),
            client_id_source: configured.as_ref().map(|id| id.source.as_str()),
            token_client_id,
            scopes: token.scopes.clone(),
            token_path: auth.token_path().display().to_string(),
            instance: paths.instance.label().to_owned(),
        }
    }
}

impl Render for AuthStatus {
    fn table_rows(&self) -> Vec<(&'static str, String)> {
        let account = match (&self.account, &self.display_name) {
            (Some(account), Some(name)) => format!("{account} ({name})"),
            (Some(account), None) => account.clone(),
            (None, _) => "(Graph returned no account name)".into(),
        };
        let client_id = match (&self.client_id, self.client_id_source) {
            (Some(id), Some(source)) => format!("{id} ({source})"),
            _ => "not configured".into(),
        };
        let mut rows = vec![
            ("Signed in as", account),
            (
                "Token expires",
                format!(
                    "{} ({})",
                    self.expires_at,
                    relative(self.expires_in_seconds)
                ),
            ),
            ("Client ID", client_id),
        ];
        if let Some(token_client_id) = &self.token_client_id {
            rows.push(("Token client ID", token_client_id.clone()));
        }
        rows.extend([
            ("Scopes", self.scopes.join(" ")),
            ("Token file", self.token_path.clone()),
            ("Instance", self.instance.clone()),
        ]);
        rows
    }
}

#[derive(Serialize)]
pub struct Logout {
    pub signed_in: bool,
    /// Whether a stored sign-in was deleted; false if there was none.
    pub removed: bool,
    pub token_path: String,
}

impl Render for Logout {
    fn table_rows(&self) -> Vec<(&'static str, String)> {
        let outcome = if self.removed {
            "Signed out."
        } else {
            "Already signed out."
        };
        vec![
            ("Status", outcome.into()),
            ("Token file", self.token_path.clone()),
        ]
    }
}

pub async fn login(paths: &Paths, auth: &Authenticator) -> Result<AuthStatus, CliError> {
    let client_id = resolve_client_id(&paths.config_file)?;
    if client_id.source == ClientIdSource::Bundled {
        eprintln!(
            "Using ms-todo's bundled app registration. To use your own, see {SETUP_GUIDE_URL}"
        );
    }
    let code = auth.start_device_flow(&client_id.value).await?;
    eprintln!(
        "To sign in, open {} and enter the code {} (it expires in {} minutes).",
        code.verification_uri,
        code.user_code,
        code.expires_in / 60
    );
    let token = auth.finish_device_flow(&client_id.value, &code).await?;
    let account = auth.account(&token).await?;
    Ok(AuthStatus::new(
        paths,
        auth,
        &token,
        &account,
        Some(client_id),
    ))
}

pub async fn status(paths: &Paths, auth: &Authenticator) -> Result<AuthStatus, CliError> {
    let token = auth.valid_token().await?;
    let account = auth.account(&token).await?;
    // Only for display: a missing or broken config must not hide the sign-in.
    let configured = resolve_client_id(&paths.config_file).ok();
    Ok(AuthStatus::new(paths, auth, &token, &account, configured))
}

pub async fn logout(auth: &Authenticator) -> Result<Logout, CliError> {
    let removed = auth.sign_out().await?;
    Ok(Logout {
        signed_in: false,
        removed,
        token_path: auth.token_path().display().to_string(),
    })
}

fn rfc3339(unix_seconds: i64) -> String {
    chrono::DateTime::from_timestamp(unix_seconds, 0)
        .map(|time| time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_else(|| unix_seconds.to_string())
}

fn relative(seconds: i64) -> String {
    if seconds <= 0 {
        return "expired; refreshes on next use".into();
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        format!("in {minutes}m")
    } else {
        format!("in {}h {}m", minutes / 60, minutes % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::relative;

    #[test]
    fn relative_expiry_reads_naturally() {
        assert_eq!(relative(59 * 60 + 30), "in 59m");
        assert_eq!(relative(90 * 60), "in 1h 30m");
        assert_eq!(relative(0), "expired; refreshes on next use");
    }
}
