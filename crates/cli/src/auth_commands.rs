//! `ms-todo auth login|status|logout`. These run without a daemon (D-033
//! item 4). From rung 1 the daemon owns refresh, and `status` will ask it.

use std::io::{BufRead, Write};

use ms_todo_core::{ErrorKind, Paths};
use ms_todo_graph::auth::{
    Account, Authenticator, ClientId, ClientIdSource, SETUP_GUIDE_URL, StoredToken,
    resolve_client_id,
};
use serde::Serialize;

use crate::error::CliError;
use crate::output::Render;
use crate::system_args::AccountType;
use crate::time::rfc3339;

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
    /// Where `auth.client_id` is read from (D-035).
    pub config_file: String,
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
            config_file: paths.config_file.display().to_string(),
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
            ("Config file", self.config_file.clone()),
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

pub fn select_account_type(requested: Option<AccountType>) -> Result<AccountType, CliError> {
    if let Some(account_type) = requested {
        return Ok(account_type);
    }
    if !crate::confirm::can_prompt() {
        return Err(CliError::message(
            ErrorKind::InvalidInput,
            "Choose an account type: run `ms-todo auth login --account-type personal` or \
             `ms-todo auth login --account-type work`. A prompt requires stdin and stderr \
             to be terminals."
                .into(),
        ));
    }
    read_account_type(&mut std::io::stdin().lock(), &mut std::io::stderr().lock())
}

fn read_account_type(
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> Result<AccountType, CliError> {
    writeln!(
        output,
        "Microsoft account: 1) Personal  2) Work or school  q) Cancel"
    )?;
    loop {
        write!(output, "Choose 1 or 2: ")?;
        output.flush()?;
        let mut answer = String::new();
        if input.read_line(&mut answer)? == 0 {
            return Err(CliError::message(
                ErrorKind::InvalidInput,
                "Sign-in cancelled: no account type was selected.".into(),
            ));
        }
        match answer.trim().to_ascii_lowercase().as_str() {
            "1" | "personal" => return Ok(AccountType::Personal),
            "2" | "work" => return Ok(AccountType::Work),
            "q" | "cancel" => {
                return Err(CliError::message(
                    ErrorKind::InvalidInput,
                    "Sign-in cancelled.".into(),
                ));
            }
            _ => writeln!(
                output,
                "Enter 1 (personal), 2 (work or school), or q to cancel."
            )?,
        }
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
    use super::{AccountType, read_account_type, relative, select_account_type};

    #[test]
    fn prompt_selects_each_account_type_without_terminal_escapes() {
        for (answer, expected) in [
            ("1\n", AccountType::Personal),
            (" PERSONAL \n", AccountType::Personal),
            ("2\n", AccountType::Work),
            ("work\n", AccountType::Work),
        ] {
            let mut output = Vec::new();
            let selected = read_account_type(&mut answer.as_bytes(), &mut output).expect("choice");
            assert_eq!(selected, expected);
            let prompt = String::from_utf8(output).expect("prompt");
            assert!(prompt.contains("Personal"));
            assert!(prompt.contains("Work or school"));
            assert!(!prompt.contains('\u{1b}'));
        }
    }

    #[test]
    fn invalid_or_empty_answer_reprompts_without_choosing_a_default() {
        let mut output = Vec::new();
        assert_eq!(
            read_account_type(&mut "\nunknown\n2\n".as_bytes(), &mut output).expect("choice"),
            AccountType::Work
        );
        let prompt = String::from_utf8(output).expect("prompt");
        assert_eq!(prompt.matches("Choose 1 or 2: ").count(), 3);
    }

    #[test]
    fn eof_and_cancel_stop_before_sign_in() {
        for answer in ["", "q\n", "cancel\n", "unknown\n"] {
            let error =
                read_account_type(&mut answer.as_bytes(), &mut Vec::new()).expect_err("cancelled");
            assert_eq!(error.kind.exit_code(), 2);
            assert!(error.message.contains("Sign-in cancelled"));
        }
    }

    #[test]
    fn explicit_account_type_needs_no_terminal() {
        for account_type in [AccountType::Personal, AccountType::Work] {
            assert_eq!(
                select_account_type(Some(account_type)).expect("explicit choice"),
                account_type
            );
        }
    }

    #[test]
    fn account_choice_selects_the_login_authority() {
        assert_eq!(
            AccountType::Personal.endpoints().authority,
            "https://login.microsoftonline.com/consumers/oauth2/v2.0"
        );
        assert_eq!(
            AccountType::Work.endpoints().authority,
            "https://login.microsoftonline.com/organizations/oauth2/v2.0"
        );
    }

    #[test]
    fn relative_expiry_reads_naturally() {
        assert_eq!(relative(59 * 60 + 30), "in 59m");
        assert_eq!(relative(90 * 60), "in 1h 30m");
        assert_eq!(relative(0), "expired; refreshes on next use");
    }
}
