/// The category of a failure, shared by every crate so the CLI can map any
/// error to the same `kind` string and exit code (docs/blueprint/07-cli.md).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// No credential is stored. Run `ms-todo auth login`.
    AuthRequired,
    /// Graph rejected the access token even after a refresh.
    AuthExpired,
    /// Microsoft rejected the refresh token (`invalid_grant`).
    AuthRevoked,
    /// Bad arguments or configuration.
    InvalidInput,
    /// The request never got a usable answer: connection, timeout, TLS.
    Network,
    /// Graph or the identity platform answered with an error.
    Api,
    /// A response body didn't have the shape we expected.
    Decode,
    /// A local failure: the token file, its lock, a bug.
    Internal,
}

impl ErrorKind {
    /// The stable `kind` value in JSON error output.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AuthRequired => "auth_required",
            Self::AuthExpired => "auth_expired",
            Self::AuthRevoked => "auth_revoked",
            Self::InvalidInput => "invalid_input",
            Self::Network => "network",
            Self::Api => "api",
            Self::Decode => "decode",
            Self::Internal => "internal",
        }
    }

    /// Process exit code, from the table in docs/blueprint/07-cli.md.
    pub fn exit_code(self) -> u8 {
        match self {
            Self::AuthRequired | Self::AuthExpired | Self::AuthRevoked => 4,
            Self::InvalidInput => 2,
            Self::Network | Self::Api | Self::Decode | Self::Internal => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ErrorKind;

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
    fn network_and_invalid_input_follow_the_cli_table() {
        assert_eq!(ErrorKind::Network.exit_code(), 1);
        assert_eq!(ErrorKind::InvalidInput.exit_code(), 2);
    }
}
