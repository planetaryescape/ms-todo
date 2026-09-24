use ms_todo_core::{ErrorKind, InvalidInstanceName, PathsError};
use ms_todo_graph::auth::AuthError;

/// A failure, flattened into what the CLI prints and the exit code it maps to.
#[derive(Debug)]
pub struct CliError {
    pub kind: ErrorKind,
    pub message: String,
    pub graph_code: Option<String>,
    pub request_id: Option<String>,
}

impl CliError {
    pub fn new(kind: ErrorKind, error: &dyn std::error::Error) -> Self {
        Self {
            kind,
            message: message_with_causes(error),
            graph_code: None,
            request_id: None,
        }
    }
}

/// "network error talking to Microsoft: error sending request: …" rather
/// than only the outermost message, which hides the actual cause.
fn message_with_causes(error: &dyn std::error::Error) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}

impl From<AuthError> for CliError {
    fn from(error: AuthError) -> Self {
        Self {
            kind: error.kind(),
            message: message_with_causes(&error),
            graph_code: error.graph_code().map(str::to_owned),
            request_id: error.request_id().map(str::to_owned),
        }
    }
}

impl From<InvalidInstanceName> for CliError {
    fn from(error: InvalidInstanceName) -> Self {
        Self::new(ErrorKind::InvalidInput, &error)
    }
}

impl From<PathsError> for CliError {
    fn from(error: PathsError) -> Self {
        Self::new(ErrorKind::Internal, &error)
    }
}

impl From<std::io::Error> for CliError {
    fn from(error: std::io::Error) -> Self {
        Self::new(ErrorKind::Internal, &error)
    }
}
