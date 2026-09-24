use std::path::PathBuf;

use ms_todo_core::ErrorKind;

use super::SETUP_GUIDE_URL;
use crate::api_error::ApiError;

const LOGIN_HINT: &str = "run `ms-todo auth login`";

/// Everything sign-in can fail with. The CLI turns each into a `kind`
/// string and exit code through [`AuthError::kind`].
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("not signed in; {LOGIN_HINT}")]
    NotSignedIn,
    #[error("Microsoft Graph rejected the access token; {LOGIN_HINT}")]
    Expired,
    #[error("the sign-in was revoked or has expired; {LOGIN_HINT}")]
    Revoked,
    #[error("the device code expired before sign-in finished; {LOGIN_HINT} again")]
    DeviceCodeExpired,
    #[error("sign-in was declined in the browser")]
    Declined,
    #[error(
        "no Entra client ID configured: set MS_TODO_CLIENT_ID or auth.client_id in {}. \
         See {SETUP_GUIDE_URL}",
        config.display()
    )]
    NoClientId { config: PathBuf },
    #[error("invalid config file {path}: {message}")]
    Config { path: PathBuf, message: String },
    /// The identity platform answered with an OAuth error, e.g. `invalid_client`.
    #[error("sign-in failed: {code}: {description}")]
    OAuth { code: String, description: String },
    /// Graph answered with a non-success status.
    #[error(transparent)]
    Api(ApiError),
    #[error("network error talking to Microsoft")]
    Network(#[source] reqwest::Error),
    #[error("unexpected response from Microsoft: {0}")]
    Decode(String),
    #[error("token store error at {path}")]
    Store {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("timed out waiting for the token lock at {0}; is another ms-todo signing in?")]
    LockTimeout(PathBuf),
}

impl AuthError {
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::NotSignedIn | Self::DeviceCodeExpired | Self::Declined => ErrorKind::AuthRequired,
            Self::Expired => ErrorKind::AuthExpired,
            Self::Revoked => ErrorKind::AuthRevoked,
            Self::NoClientId { .. } | Self::Config { .. } => ErrorKind::InvalidInput,
            Self::OAuth { .. } | Self::Api(_) => ErrorKind::Api,
            Self::Network(_) => ErrorKind::Network,
            Self::Decode(_) => ErrorKind::Decode,
            Self::Store { .. } | Self::LockTimeout(_) => ErrorKind::Internal,
        }
    }

    /// The error code from the identity platform or Graph, if there was one.
    pub fn graph_code(&self) -> Option<&str> {
        match self {
            Self::OAuth { code, .. } => Some(code),
            Self::Api(error) => Some(&error.code),
            _ => None,
        }
    }

    /// Graph's `request-id` header, for support requests.
    pub fn request_id(&self) -> Option<&str> {
        match self {
            Self::Api(error) => error.request_id.as_deref(),
            _ => None,
        }
    }

    pub(crate) fn store(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Store {
            path: path.into(),
            source,
        }
    }
}

impl From<reqwest::Error> for AuthError {
    fn from(error: reqwest::Error) -> Self {
        if error.is_decode() {
            Self::Decode(error.to_string())
        } else {
            Self::Network(error)
        }
    }
}
