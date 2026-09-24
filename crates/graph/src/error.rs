// Shape adapted from spotuify crates/spotuify-spotify/src/error.rs
// (`SpotifyError` and its kind mapping) @ d807e5e4f9d2f09878cdc22309af3589623f7785.
// Graph's status-specific cases (403, 404, 409, 412) share one `Api` variant
// and are told apart by `kind()`, because each needs the same code, message
// and request ID anyway.

use std::time::Duration;

use ms_todo_core::ErrorKind;

use crate::api_error::ApiError;
use crate::auth::AuthError;

/// Everything a Graph call can fail with (docs/blueprint/03-graph-provider.md).
#[derive(Debug, thiserror::Error)]
pub enum GraphError {
    /// Signed out, revoked, or the token was rejected even after a refresh.
    #[error(transparent)]
    Auth(#[from] AuthError),
    /// Graph answered with an error status, after any retries.
    #[error(transparent)]
    Api(ApiError),
    /// Graph kept answering 429, or asked for a longer wait than a request
    /// may sleep for.
    #[error("Microsoft Graph is rate limiting ms-todo{}", wait_hint(*.retry_after))]
    RateLimited {
        retry_after: Option<Duration>,
        #[source]
        source: ApiError,
    },
    /// No usable answer: connection, TLS or the 60-second timeout.
    #[error("network error talking to Microsoft Graph")]
    Network(#[source] reqwest::Error),
    #[error("unexpected response from Microsoft Graph: {0}")]
    Decode(String),
    #[error("{0}")]
    InvalidInput(String),
    /// A create, a recurring completion or a raw write may have reached
    /// Graph, but no answer says whether it ran: a timeout, a 5xx or 408, or
    /// a connection lost after sending. It's never resent automatically
    /// (D-028).
    #[error("Microsoft Graph may or may not have applied this change")]
    OutcomeUnknown(#[source] Box<GraphError>),
    /// Pagination hit the safety cap. Returned instead of partial results.
    #[error("stopped after {pages} pages of {path}; refusing to return a partial collection")]
    PageLimit { path: String, pages: usize },
}

fn wait_hint(retry_after: Option<Duration>) -> String {
    retry_after
        .map(|wait| format!("; try again in {} seconds", wait.as_secs().max(1)))
        .unwrap_or_default()
}

impl GraphError {
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::Auth(error) => error.kind(),
            Self::Api(error) => match error.status {
                404 => ErrorKind::NotFound,
                409 | 412 => ErrorKind::Conflict,
                429 => ErrorKind::RateLimited,
                400..=499 => ErrorKind::Rejected,
                _ => ErrorKind::Api,
            },
            Self::RateLimited { .. } => ErrorKind::RateLimited,
            Self::Network(_) => ErrorKind::Network,
            Self::Decode(_) => ErrorKind::Decode,
            Self::InvalidInput(_) => ErrorKind::InvalidInput,
            Self::OutcomeUnknown(_) => ErrorKind::OutcomeUnknown,
            Self::PageLimit { .. } => ErrorKind::Internal,
        }
    }

    /// The HTTP status Graph answered with, if it answered.
    pub fn status(&self) -> Option<u16> {
        self.api_error().map(|error| error.status)
    }

    fn api_error(&self) -> Option<&ApiError> {
        match self {
            Self::Api(error) | Self::RateLimited { source: error, .. } => Some(error),
            Self::OutcomeUnknown(cause) => cause.api_error(),
            _ => None,
        }
    }

    /// Whether Graph rejected a delta link, so the scope must be read whole
    /// again: 410 `SyncStateNotFound` with any inner code, or 400 "Badly
    /// formed token." (S4). A 404 or 5xx on a delta request isn't one: the
    /// same link answered 200 a minute later.
    pub fn is_delta_reset(&self) -> bool {
        match self.api_error() {
            Some(error) => {
                error.status == 410
                    || (error.status == 400 && error.message.contains("Badly formed token"))
            }
            None => false,
        }
    }

    /// Graph's `error.code`, such as `ErrorItemNotFound`.
    pub fn graph_code(&self) -> Option<&str> {
        match self {
            Self::Auth(error) => error.graph_code(),
            _ => self.api_error().map(|error| error.code.as_str()),
        }
    }

    /// Graph's `request-id` header, for support requests.
    pub fn request_id(&self) -> Option<&str> {
        match self {
            Self::Auth(error) => error.request_id(),
            _ => self
                .api_error()
                .and_then(|error| error.request_id.as_deref()),
        }
    }
}

impl From<reqwest::Error> for GraphError {
    fn from(error: reqwest::Error) -> Self {
        if error.is_decode() {
            Self::Decode(error.to_string())
        } else {
            Self::Network(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api(status: u16) -> GraphError {
        GraphError::Api(ApiError::parse(status, Some("rid".into()), "{}"))
    }

    #[test]
    fn statuses_map_to_the_cli_kinds() {
        assert_eq!(api(404).kind(), ErrorKind::NotFound);
        assert_eq!(api(412).kind(), ErrorKind::Conflict);
        assert_eq!(api(409).kind(), ErrorKind::Conflict);
        assert_eq!(api(403).kind(), ErrorKind::Rejected);
        assert_eq!(api(400).kind(), ErrorKind::Rejected);
        assert_eq!(api(503).kind(), ErrorKind::Api);
        assert_eq!(api(404).request_id(), Some("rid"));
    }

    #[test]
    fn an_unknown_outcome_keeps_what_graph_said() {
        let error = GraphError::OutcomeUnknown(Box::new(api(503)));
        assert_eq!(error.kind(), ErrorKind::OutcomeUnknown);
        assert_eq!(error.request_id(), Some("rid"));
        assert_eq!(error.status(), Some(503));
    }

    #[test]
    fn only_a_410_or_a_badly_formed_token_resets_a_delta_link() {
        let gone = GraphError::Api(ApiError::parse(
            410,
            None,
            r#"{"error":{"code":"SyncStateNotFound","message":"The delta token is no longer valid, and the app must reset the sync state.","innerError":{"code":"SyncStateInvalid"}}}"#,
        ));
        let badly_formed = GraphError::Api(ApiError::parse(
            400,
            None,
            r#"{"error":{"code":"BadRequest","message":"Badly formed token."}}"#,
        ));
        let other_400 = GraphError::Api(ApiError::parse(
            400,
            None,
            r#"{"error":{"code":"BadRequest","message":"Skip token is not provided."}}"#,
        ));
        assert!(gone.is_delta_reset());
        assert!(badly_formed.is_delta_reset());
        assert!(!other_400.is_delta_reset());
        assert!(!api(404).is_delta_reset());
        assert!(!api(500).is_delta_reset());
    }

    #[test]
    fn auth_failures_keep_their_kind() {
        let error = GraphError::from(AuthError::Revoked);
        assert_eq!(error.kind(), ErrorKind::AuthRevoked);
    }
}
