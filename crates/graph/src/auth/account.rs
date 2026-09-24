//! `GET /me`: who the token belongs to.

use serde::{Deserialize, Serialize};

use super::AuthError;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub user_principal_name: Option<String>,
    pub mail: Option<String>,
    pub display_name: Option<String>,
}

impl Account {
    /// `userPrincipalName`, or `mail` when Graph leaves the UPN out.
    pub fn sign_in_name(&self) -> Option<&str> {
        self.user_principal_name
            .as_deref()
            .or(self.mail.as_deref())
            .filter(|name| !name.is_empty())
    }
}

#[derive(Deserialize)]
struct GraphErrorBody {
    error: GraphErrorDetail,
}

#[derive(Deserialize)]
struct GraphErrorDetail {
    code: String,
    #[serde(default)]
    message: String,
}

pub(crate) async fn fetch_me(
    http: &reqwest::Client,
    graph: &str,
    access_token: &str,
) -> Result<Account, AuthError> {
    let response = http
        .get(format!("{graph}/me"))
        .bearer_auth(access_token)
        .send()
        .await?;
    let status = response.status();
    if status.is_success() {
        return Ok(response.json().await?);
    }
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(AuthError::Expired);
    }
    let request_id = response
        .headers()
        .get("request-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = response.text().await.unwrap_or_default();
    let (code, message) = match serde_json::from_str::<GraphErrorBody>(&body) {
        Ok(parsed) => (parsed.error.code, parsed.error.message),
        Err(_) => ("unknown".to_owned(), body.chars().take(200).collect()),
    };
    Err(AuthError::Api {
        status: status.as_u16(),
        code,
        message,
        request_id,
    })
}
