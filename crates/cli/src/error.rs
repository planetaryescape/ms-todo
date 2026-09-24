use ms_todo_core::{ErrorKind, InvalidInstanceName, PathsError, message_with_causes};
use ms_todo_graph::auth::AuthError;
use ms_todo_protocol::{ErrorPayload, ListRef};

/// A failure, flattened into what the CLI prints and the exit code it maps to.
#[derive(Debug)]
pub struct CliError {
    pub kind: ErrorKind,
    pub message: String,
    pub graph_code: Option<String>,
    pub request_id: Option<String>,
    /// The lists an ambiguous `--list` name matched.
    pub candidates: Vec<ListRef>,
    /// Whoever read stdout stopped, as with `ms-todo tasks list | head`.
    /// That's the reader's choice, not a failure, so nothing is reported.
    pub stdout_closed: bool,
}

impl CliError {
    pub fn new(kind: ErrorKind, error: &dyn std::error::Error) -> Self {
        Self::message(kind, message_with_causes(error))
    }

    pub fn message(kind: ErrorKind, message: String) -> Self {
        Self {
            kind,
            message,
            graph_code: None,
            request_id: None,
            candidates: Vec::new(),
            stdout_closed: false,
        }
    }
}

impl From<AuthError> for CliError {
    fn from(error: AuthError) -> Self {
        Self {
            graph_code: error.graph_code().map(str::to_owned),
            request_id: error.request_id().map(str::to_owned),
            ..Self::new(error.kind(), &error)
        }
    }
}

/// The daemon's answer to a failed request.
impl From<ErrorPayload> for CliError {
    fn from(error: ErrorPayload) -> Self {
        Self {
            // A kind from a newer daemon still fails, just less specifically.
            kind: ErrorKind::parse(&error.kind).unwrap_or(ErrorKind::Internal),
            message: error.message,
            graph_code: error.graph_code,
            request_id: error.request_id,
            candidates: error.candidates,
            stdout_closed: false,
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
        Self {
            stdout_closed: error.kind() == std::io::ErrorKind::BrokenPipe,
            ..Self::new(ErrorKind::Internal, &error)
        }
    }
}
