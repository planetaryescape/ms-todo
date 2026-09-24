//! The token endpoint's answer, shared by the device-code poll and refresh.

use serde::Deserialize;

use super::AuthError;
use super::token_store::StoredToken;

#[derive(Debug, Deserialize)]
pub(crate) struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
    scope: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) error_description: Option<String>,
}

impl TokenResponse {
    pub(crate) fn oauth_error(&self) -> Option<AuthError> {
        self.error.as_ref().map(|code| AuthError::OAuth {
            code: code.clone(),
            description: self.error_description.clone().unwrap_or_default(),
        })
    }

    /// Build the whole credential. On refresh, `previous` supplies the
    /// refresh token and scopes when Microsoft leaves them out.
    pub(crate) fn into_token(
        self,
        client_id: &str,
        previous: Option<&StoredToken>,
    ) -> Result<StoredToken, AuthError> {
        let access_token = self
            .access_token
            .ok_or_else(|| AuthError::Decode("no access_token in token response".into()))?;
        let refresh_token = self
            .refresh_token
            .or_else(|| previous.map(|token| token.refresh_token.clone()))
            .ok_or_else(|| {
                AuthError::Decode(
                    "no refresh_token in token response; was offline_access granted?".into(),
                )
            })?;
        let scopes = match self.scope {
            Some(scope) => scope.split_whitespace().map(str::to_owned).collect(),
            None => previous
                .map(|token| token.scopes.clone())
                .unwrap_or_default(),
        };
        Ok(StoredToken {
            access_token,
            refresh_token,
            expires_at: chrono::Utc::now().timestamp() + self.expires_in.unwrap_or(3600),
            scopes,
            client_id: client_id.to_owned(),
        })
    }
}
