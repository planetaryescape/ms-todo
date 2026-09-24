// Adapted from mxr crates/provider-outlook/src/auth.rs @ dfb23d10138b1cfc24f8ea7450d3426e5e4da37a
// Changes: `/common` instead of the consumers/organizations split, Graph
// scopes, a local deadline from `expires_in`, and a configurable poll unit so
// tests don't wait real seconds.

use crate::api_error::{ApiError, RAW_BODY_EXCERPT};
use std::time::{Duration, Instant};

use serde::Deserialize;

use super::token_response::TokenResponse;
use super::token_store::StoredToken;
use super::{AuthError, SCOPES};

/// Consecutive transport failures tolerated while polling the token
/// endpoint. RFC 8628 polling is designed to tolerate transient failures: a
/// single timed-out poll must not abandon a device code the user is still
/// typing in (mxr plans/006-auth-poll-transient-errors.md).
pub(crate) const DEVICE_POLL_MAX_TRANSPORT_FAILURES: u32 = 6;

/// RFC 8628 §3.5: on `slow_down`, add 5 seconds to the interval.
const SLOW_DOWN_STEP: u32 = 5;
/// RFC 8628 §3.2: 5 seconds when the server doesn't say.
const DEFAULT_INTERVAL: u64 = 5;

/// What the user needs to finish sign-in in a browser.
#[derive(Clone, Debug, Deserialize)]
pub struct DeviceCode {
    pub(crate) device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    /// Seconds until `user_code` stops working.
    pub expires_in: u64,
    pub interval: Option<u64>,
}

pub(crate) async fn start(
    http: &reqwest::Client,
    authority: &str,
    client_id: &str,
) -> Result<DeviceCode, AuthError> {
    let response = http
        .post(format!("{authority}/devicecode"))
        .form(&[("client_id", client_id), ("scope", SCOPES)])
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        // The identity platform explains failures (bad client ID, public
        // flows disabled) in an OAuth error body; surface that, not the status.
        return Err(serde_json::from_str::<TokenResponse>(&body)
            .ok()
            .and_then(|parsed| parsed.oauth_error())
            .unwrap_or_else(|| {
                AuthError::Api(ApiError {
                    status: status.as_u16(),
                    code: "device_code_request_failed".into(),
                    message: body.chars().take(RAW_BODY_EXCERPT).collect(),
                    request_id: None,
                })
            }));
    }
    serde_json::from_str(&body).map_err(|error| AuthError::Decode(error.to_string()))
}

/// Poll the token endpoint until sign-in completes, is declined, or the
/// code expires. Returns the token; the caller saves it.
pub(crate) async fn poll(
    http: &reqwest::Client,
    authority: &str,
    client_id: &str,
    code: &DeviceCode,
    unit: Duration,
) -> Result<StoredToken, AuthError> {
    let mut interval =
        u32::try_from(code.interval.unwrap_or(DEFAULT_INTERVAL).max(1)).unwrap_or(u32::MAX);
    let deadline =
        Instant::now() + unit.saturating_mul(u32::try_from(code.expires_in).unwrap_or(u32::MAX));
    let mut transport_failures = 0u32;

    loop {
        tokio::time::sleep(unit.saturating_mul(interval)).await;
        if Instant::now() >= deadline {
            return Err(AuthError::DeviceCodeExpired);
        }

        let sent = http
            .post(format!("{authority}/token"))
            .form(&[
                ("client_id", client_id),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", code.device_code.as_str()),
            ])
            .send()
            .await;
        // Transient transport failures (including the request timeout) must
        // not abandon a device code the user is mid-way through approving.
        // The server tells us definitively when the code is dead.
        let response = match sent {
            Ok(response) => response.json::<TokenResponse>().await,
            Err(error) => Err(error),
        };
        let response = match response {
            Ok(response) => {
                transport_failures = 0;
                response
            }
            Err(error) => {
                transport_failures += 1;
                if transport_failures >= DEVICE_POLL_MAX_TRANSPORT_FAILURES {
                    return Err(AuthError::Network(error));
                }
                continue;
            }
        };

        let Some(error) = response.error.clone() else {
            return response.into_token(client_id, None);
        };
        match error.as_str() {
            "authorization_pending" => {}
            "slow_down" => interval += SLOW_DOWN_STEP,
            // `authorization_declined` is RFC 8628's name; Entra sends `access_denied`.
            "authorization_declined" | "access_denied" => return Err(AuthError::Declined),
            "expired_token" | "bad_verification_code" => return Err(AuthError::DeviceCodeExpired),
            _ => {
                return Err(AuthError::OAuth {
                    code: error,
                    description: response.error_description.unwrap_or_default(),
                });
            }
        }
    }
}
