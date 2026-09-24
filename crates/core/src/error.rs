/// The category of a failure, shared by every crate so the CLI can map any
/// error to the same `kind` string and exit code (docs/blueprint/07-cli.md).
/// The daemon sends the `kind` string over IPC, so the strings are part of
/// the protocol: renaming one is a breaking change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// No credential is stored. Run `ms-todo auth login`.
    AuthRequired,
    /// Graph rejected the access token even after a refresh.
    AuthExpired,
    /// Microsoft rejected the refresh token (`invalid_grant`).
    AuthRevoked,
    /// Bad arguments or configuration, or a name that matches several things.
    InvalidInput,
    /// The list, task or path doesn't exist.
    NotFound,
    /// Someone else changed it first: 409, or 412 on `If-Match`.
    Conflict,
    /// Graph refused the request for good: 403, or another permanent 4xx.
    Rejected,
    /// Graph kept answering 429 after the retries ran out.
    RateLimited,
    /// The API, or this version of ms-todo, can't do it.
    Unsupported,
    /// The request never got a usable answer: connection, timeout, TLS.
    Network,
    /// Graph or the identity platform answered with an error, such as a 5xx
    /// after the retries.
    Api,
    /// A response body didn't have the shape we expected.
    Decode,
    /// The daemon couldn't be started or reached, or speaks another protocol.
    DaemonUnavailable,
    /// A local failure: the token file, its lock, a bug.
    Internal,
}

impl ErrorKind {
    /// The stable `kind` value in JSON error output and on the IPC wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AuthRequired => "auth_required",
            Self::AuthExpired => "auth_expired",
            Self::AuthRevoked => "auth_revoked",
            Self::InvalidInput => "invalid_input",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::Rejected => "rejected",
            Self::RateLimited => "rate_limited",
            Self::Unsupported => "unsupported",
            Self::Network => "network",
            Self::Api => "api",
            Self::Decode => "decode",
            Self::DaemonUnavailable => "daemon_unavailable",
            Self::Internal => "internal",
        }
    }

    /// The inverse of [`Self::as_str`]. `None` for a kind this build doesn't
    /// know, such as one a newer daemon sent.
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "auth_required" => Self::AuthRequired,
            "auth_expired" => Self::AuthExpired,
            "auth_revoked" => Self::AuthRevoked,
            "invalid_input" => Self::InvalidInput,
            "not_found" => Self::NotFound,
            "conflict" => Self::Conflict,
            "rejected" => Self::Rejected,
            "rate_limited" => Self::RateLimited,
            "unsupported" => Self::Unsupported,
            "network" => Self::Network,
            "api" => Self::Api,
            "decode" => Self::Decode,
            "daemon_unavailable" => Self::DaemonUnavailable,
            "internal" => Self::Internal,
            _ => return None,
        })
    }

    /// Process exit code, from the table in docs/blueprint/07-cli.md.
    pub fn exit_code(self) -> u8 {
        match self {
            Self::InvalidInput => 2,
            Self::NotFound => 3,
            Self::AuthRequired | Self::AuthExpired | Self::AuthRevoked => 4,
            Self::Conflict | Self::Rejected => 5,
            Self::RateLimited => 6,
            Self::Unsupported => 7,
            Self::Network | Self::Api | Self::Decode | Self::DaemonUnavailable | Self::Internal => {
                1
            }
        }
    }
}

/// An error's message followed by each of its causes, so "network error
/// talking to Microsoft Graph: error sending request: …" shows the actual
/// cause rather than only the outermost message.
pub fn message_with_causes(error: &dyn std::error::Error) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}

#[cfg(test)]
mod tests {
    use super::ErrorKind;

    const ALL: [ErrorKind; 14] = [
        ErrorKind::AuthRequired,
        ErrorKind::AuthExpired,
        ErrorKind::AuthRevoked,
        ErrorKind::InvalidInput,
        ErrorKind::NotFound,
        ErrorKind::Conflict,
        ErrorKind::Rejected,
        ErrorKind::RateLimited,
        ErrorKind::Unsupported,
        ErrorKind::Network,
        ErrorKind::Api,
        ErrorKind::Decode,
        ErrorKind::DaemonUnavailable,
        ErrorKind::Internal,
    ];

    #[test]
    fn every_sign_in_failure_exits_4() {
        for kind in [
            ErrorKind::AuthRequired,
            ErrorKind::AuthExpired,
            ErrorKind::AuthRevoked,
        ] {
            assert_eq!(kind.exit_code(), 4, "{}", kind.as_str());
        }
    }

    #[test]
    fn exit_codes_follow_the_cli_table() {
        assert_eq!(ErrorKind::Network.exit_code(), 1);
        assert_eq!(ErrorKind::InvalidInput.exit_code(), 2);
        assert_eq!(ErrorKind::NotFound.exit_code(), 3);
        assert_eq!(ErrorKind::Conflict.exit_code(), 5);
        assert_eq!(ErrorKind::Rejected.exit_code(), 5);
        assert_eq!(ErrorKind::RateLimited.exit_code(), 6);
        assert_eq!(ErrorKind::Unsupported.exit_code(), 7);
    }

    #[test]
    fn kind_strings_round_trip_and_are_unique() {
        for kind in ALL {
            assert_eq!(ErrorKind::parse(kind.as_str()), Some(kind));
        }
        let mut strings: Vec<_> = ALL.iter().map(|kind| kind.as_str()).collect();
        strings.sort_unstable();
        strings.dedup();
        assert_eq!(strings.len(), ALL.len());
        assert_eq!(ErrorKind::parse("from_a_newer_daemon"), None);
    }
}
