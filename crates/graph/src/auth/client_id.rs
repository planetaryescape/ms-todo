//! Which Entra app ms-todo signs in as (D-025). Highest priority first:
//! `MS_TODO_CLIENT_ID`, then `auth.client_id` in config.toml, then the ID
//! baked into release builds.

use std::io::ErrorKind;
use std::path::Path;

use serde::Deserialize;

use super::AuthError;

pub const CLIENT_ID_ENV: &str = "MS_TODO_CLIENT_ID";

/// Set by the release workflow; `None` in local builds unless the builder
/// exported `MS_TODO_CLIENT_ID` (mxr's `BUNDLED_CLIENT_ID` pattern).
pub const BUNDLED_CLIENT_ID: Option<&str> = option_env!("MS_TODO_CLIENT_ID");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientIdSource {
    Env,
    Config,
    Bundled,
}

impl ClientIdSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Env => "env",
            Self::Config => "config",
            Self::Bundled => "bundled",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientId {
    pub value: String,
    pub source: ClientIdSource,
}

/// Resolve from the real environment, config file and build.
pub fn resolve_client_id(config_file: &Path) -> Result<ClientId, AuthError> {
    let env = std::env::var(CLIENT_ID_ENV).ok();
    resolve_client_id_from(env.as_deref(), config_file, BUNDLED_CLIENT_ID)
}

/// The resolution rule with its inputs passed in, so tests don't need to
/// mutate the process environment.
pub fn resolve_client_id_from(
    env: Option<&str>,
    config_file: &Path,
    bundled: Option<&str>,
) -> Result<ClientId, AuthError> {
    let found = |value: &str, source| ClientId {
        value: value.trim().to_owned(),
        source,
    };
    if let Some(value) = non_blank(env) {
        return Ok(found(value, ClientIdSource::Env));
    }
    if let Some(value) = read_config_client_id(config_file)? {
        return Ok(found(&value, ClientIdSource::Config));
    }
    if let Some(value) = non_blank(bundled) {
        return Ok(found(value, ClientIdSource::Bundled));
    }
    Err(AuthError::NoClientId {
        config: config_file.to_path_buf(),
    })
}

fn non_blank(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.trim().is_empty())
}

#[derive(Deserialize, Default)]
struct ConfigFile {
    #[serde(default)]
    auth: AuthSection,
}

#[derive(Deserialize, Default)]
struct AuthSection {
    client_id: Option<String>,
}

fn read_config_client_id(path: &Path) -> Result<Option<String>, AuthError> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(AuthError::Config {
                path: path.to_path_buf(),
                message: error.to_string(),
            });
        }
    };
    let config: ConfigFile = toml::from_str(&raw).map_err(|error| AuthError::Config {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    Ok(non_blank(config.auth.client_id.as_deref()).map(str::to_owned))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with(contents: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("config.toml"), contents).expect("write config");
        dir
    }

    #[test]
    fn env_wins_over_config_and_bundled() {
        let dir = config_with("[auth]\nclient_id = \"from-config\"\n");
        let id = resolve_client_id_from(
            Some("from-env"),
            &dir.path().join("config.toml"),
            Some("bundled"),
        )
        .expect("resolved");
        assert_eq!(id.value, "from-env");
        assert_eq!(id.source, ClientIdSource::Env);
    }

    #[test]
    fn config_wins_over_bundled() {
        let dir = config_with("[auth]\nclient_id = \" from-config \"\n");
        let id = resolve_client_id_from(None, &dir.path().join("config.toml"), Some("bundled"))
            .expect("resolved");
        assert_eq!(id.value, "from-config");
        assert_eq!(id.source, ClientIdSource::Config);
    }

    #[test]
    fn falls_back_to_the_bundled_id_when_nothing_else_is_set() {
        let dir = tempfile::tempdir().expect("tempdir");
        let id = resolve_client_id_from(Some("  "), &dir.path().join("missing.toml"), Some("b"))
            .expect("resolved");
        assert_eq!(id.source, ClientIdSource::Bundled);
    }

    #[test]
    fn nothing_set_is_a_clear_error_pointing_at_the_guide() {
        let dir = tempfile::tempdir().expect("tempdir");
        let error = resolve_client_id_from(None, &dir.path().join("missing.toml"), None)
            .expect_err("no client id");
        assert!(matches!(error, AuthError::NoClientId { .. }));
        assert!(error.to_string().contains("entra-app-registration.md"));
    }

    #[test]
    fn a_malformed_config_is_reported_not_ignored() {
        let dir = config_with("[auth\n");
        let error = resolve_client_id_from(None, &dir.path().join("config.toml"), Some("b"))
            .expect_err("bad toml");
        assert!(matches!(error, AuthError::Config { .. }));
    }
}
