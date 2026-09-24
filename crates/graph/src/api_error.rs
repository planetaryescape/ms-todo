//! Graph's error response: the `{ "error": { "code", "message" } }` body and
//! the `request-id` header (docs/blueprint/03-graph-provider.md#http-client).
//! Moved here from `auth/account.rs` so sign-in and the HTTP client parse
//! errors the same way.

use serde::Deserialize;

/// A non-success answer from Graph, with what support needs to trace it.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("Microsoft Graph returned {status}: {code}: {message}")]
pub struct ApiError {
    pub status: u16,
    /// Graph's `error.code`, or `unknown` when the body wasn't Graph's shape.
    pub code: String,
    pub message: String,
    /// Graph's `request-id` response header.
    pub request_id: Option<String>,
}

#[derive(Deserialize)]
struct Body {
    error: Detail,
}

#[derive(Deserialize)]
struct Detail {
    code: String,
    #[serde(default)]
    message: String,
}

/// Longest slice of a non-Graph body kept in the message, so an HTML error
/// page from a proxy can't flood the terminal.
pub(crate) const RAW_BODY_EXCERPT: usize = 200;

impl ApiError {
    /// Consume a non-success response. Never fails: a body we can't read or
    /// parse still becomes an error with the status and an excerpt.
    pub(crate) async fn from_response(response: reqwest::Response) -> Self {
        let status = response.status().as_u16();
        let request_id = request_id(response.headers());
        let body = response.text().await.unwrap_or_default();
        Self::parse(status, request_id, &body)
    }

    pub(crate) fn parse(status: u16, request_id: Option<String>, body: &str) -> Self {
        let (code, message) = match serde_json::from_str::<Body>(body) {
            Ok(parsed) => (parsed.error.code, parsed.error.message),
            Err(_) => (
                "unknown".to_owned(),
                body.chars().take(RAW_BODY_EXCERPT).collect(),
            ),
        };
        Self {
            status,
            code,
            message,
            request_id,
        }
    }
}

pub(crate) fn request_id(headers: &reqwest::header::HeaderMap) -> Option<String> {
    headers
        .get("request-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::ApiError;

    #[test]
    fn parses_graphs_error_body() {
        let error = ApiError::parse(
            400,
            Some("rid".into()),
            r#"{"error":{"code":"ErrorInvalidIdMalformed","message":"Id is malformed.","innerError":{}}}"#,
        );
        assert_eq!(error.code, "ErrorInvalidIdMalformed");
        assert_eq!(error.message, "Id is malformed.");
        assert_eq!(error.request_id.as_deref(), Some("rid"));
    }

    #[test]
    fn keeps_an_excerpt_of_a_body_that_is_not_graphs() {
        let html = format!("<html>{}</html>", "x".repeat(1000));
        let error = ApiError::parse(502, None, &html);
        assert_eq!(error.code, "unknown");
        assert_eq!(error.message.chars().count(), 200);
    }
}
