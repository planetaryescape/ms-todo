//! Refresh as a compare-and-swap (docs/blueprint/03-graph-provider.md#sign-in;
//! vault: `Refresh Token Rotation Is Shared State`). Microsoft replaces the
//! refresh token on each use, so two processes refreshing from the same old
//! token would leave one of them holding a revoked token. Under the lock we
//! re-read the file and only spend the refresh token if nobody else has.

use super::token_response::TokenResponse;
use super::token_store::{StoredToken, TokenStore};
use super::{AuthError, SCOPES};

pub(crate) async fn refresh_compare_and_swap(
    http: &reqwest::Client,
    authority: &str,
    store: &TokenStore,
    stale: &StoredToken,
) -> Result<StoredToken, AuthError> {
    let lock = store.lock().await?;
    let current = store.load()?.ok_or(AuthError::NotSignedIn)?;
    if current.refresh_token != stale.refresh_token || !current.is_near_expiry() {
        // Another process refreshed, or signed in again, while we waited.
        return Ok(current);
    }

    let response = http
        .post(format!("{authority}/token"))
        .form(&[
            ("client_id", current.client_id.as_str()),
            ("grant_type", "refresh_token"),
            ("refresh_token", current.refresh_token.as_str()),
            ("scope", SCOPES),
        ])
        .send()
        .await?
        .json::<TokenResponse>()
        .await?;

    if response.error.as_deref() == Some("invalid_grant") {
        // Delete only the credential that failed. If the file now holds a
        // different refresh token, a newer sign-in landed and must survive.
        let still_ours = store
            .load()?
            .is_some_and(|stored| stored.refresh_token == current.refresh_token);
        if still_ours {
            store.delete(&lock)?;
        }
        return Err(AuthError::Revoked);
    }
    if let Some(error) = response.oauth_error() {
        return Err(error);
    }

    // Microsoft usually rotates the refresh token, but may not; keep the old one if so.
    let refreshed = response.into_token(&current.client_id, Some(&current))?;
    store.save(&lock, &refreshed)?;
    Ok(refreshed)
}
