//! `GET /me`: who the token belongs to.

use serde::{Deserialize, Serialize};

use super::AuthError;
use crate::api_error::ApiError;

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
    Err(AuthError::Api(ApiError::from_response(response).await))
}
