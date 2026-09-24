// Adapted from spotuify crates/spotuify-protocol/src/paths.rs @ d807e5e4f9d2f09878cdc22309af3589623f7785
// (instance naming and the Cargo `target/` detection only).

//! Where ms-todo keeps its files (docs/blueprint/01-architecture.md#files).
//!
//! Dev and installed builds are kept apart by an instance name, so an agent
//! running a local build never touches the installed binary's sign-in:
//!
//! - `MS_TODO_INSTANCE=<name>` (or `--instance <name>`) uses `ms-todo-<name>`.
//! - A binary run from Cargo's `target/` tree, or any debug build, defaults to
//!   the `dev` instance, so its data lives in `ms-todo-dev`.
//! - Everything else uses plain `ms-todo`.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

pub const APP_NAME: &str = "ms-todo";
pub const INSTANCE_ENV: &str = "MS_TODO_INSTANCE";
const DEV_INSTANCE: &str = "dev";

/// Which copy of ms-todo's data a process uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Instance {
    /// The installed build: `<data_dir>/ms-todo`.
    Default,
    /// `<data_dir>/ms-todo-<name>`.
    Named(String),
}

#[derive(Debug, thiserror::Error)]
#[error("invalid instance name {0:?}: use letters, digits, '-' or '_' (at most 64 characters)")]
pub struct InvalidInstanceName(pub String);

impl Instance {
    /// Resolve from `--instance`, then `MS_TODO_INSTANCE`, then the build kind.
    pub fn detect(flag: Option<&str>) -> Result<Self, InvalidInstanceName> {
        let env = std::env::var(INSTANCE_ENV).ok();
        Self::resolve(
            flag,
            env.as_deref(),
            cfg!(debug_assertions) || current_exe_is_cargo_target_build(),
        )
    }

    /// Pure resolution rule, separated from the environment for tests.
    pub fn resolve(
        flag: Option<&str>,
        env: Option<&str>,
        is_target_build: bool,
    ) -> Result<Self, InvalidInstanceName> {
        match flag.or(env).filter(|name| !name.is_empty()) {
            Some(name) => validate(name).map(|()| Self::Named(name.to_owned())),
            None if is_target_build => Ok(Self::Named(DEV_INSTANCE.to_owned())),
            None => Ok(Self::Default),
        }
    }

    /// The directory name under the platform data directory.
    pub fn dir_name(&self) -> String {
        match self {
            Self::Default => APP_NAME.to_owned(),
            Self::Named(name) => format!("{APP_NAME}-{name}"),
        }
    }

    /// The name shown to users; `default` for the installed instance.
    pub fn label(&self) -> &str {
        match self {
            Self::Default => "default",
            Self::Named(name) => name,
        }
    }
}

// The name becomes a path component, so it must not be able to escape the
// data directory.
fn validate(name: &str) -> Result<(), InvalidInstanceName> {
    let ok = name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err(InvalidInstanceName(name.to_owned()))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PathsError {
    #[error("cannot find the {0} directory for this user (is HOME set?)")]
    NoPlatformDir(&'static str),
}

/// Resolved file locations for one instance.
#[derive(Clone, Debug)]
pub struct Paths {
    pub instance: Instance,
    /// `<data_dir>/ms-todo[-<instance>]`
    pub data_dir: PathBuf,
    /// `<config_dir>/ms-todo/config.toml`. Not per instance: it only holds
    /// settings that are the same for every build, such as `auth.client_id`.
    pub config_file: PathBuf,
}

impl Paths {
    pub fn resolve(instance: Instance) -> Result<Self, PathsError> {
        let data_base = dirs::data_dir().ok_or(PathsError::NoPlatformDir("data"))?;
        let config_base = dirs::config_dir().ok_or(PathsError::NoPlatformDir("config"))?;
        Ok(Self::under(instance, &data_base, &config_base))
    }

    pub fn under(instance: Instance, data_base: &Path, config_base: &Path) -> Self {
        Self {
            data_dir: data_base.join(instance.dir_name()),
            config_file: config_base.join(APP_NAME).join("config.toml"),
            instance,
        }
    }

    /// Kept 0700; holds `token.json` (0600) and its lock file.
    pub fn auth_dir(&self) -> PathBuf {
        self.data_dir.join("auth")
    }
}

fn current_exe_is_cargo_target_build() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    path_has_cargo_target_profile_ancestor(&exe)
}

fn path_has_cargo_target_profile_ancestor(path: &Path) -> bool {
    path.ancestors().skip(1).any(|dir| {
        let is_profile = dir
            .file_name()
            .is_some_and(|name| name == OsStr::new("debug") || name == OsStr::new("release"));
        is_profile && has_target_parent(dir)
    })
}

// `target/release` or, when cross-compiling, `target/<triple>/release`.
fn has_target_parent(profile_dir: &Path) -> bool {
    profile_dir
        .ancestors()
        .skip(1)
        .take(2)
        .any(|dir| dir.file_name().is_some_and(is_cargo_target_dir_name))
}

fn is_cargo_target_dir_name(name: &OsStr) -> bool {
    let name = name.to_string_lossy();
    name == "target" || name.starts_with("target-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_builds_default_to_the_dev_instance() {
        let instance = Instance::resolve(None, None, true).expect("valid");
        assert_eq!(instance.dir_name(), "ms-todo-dev");
    }

    #[test]
    fn installed_builds_use_the_plain_name() {
        let instance = Instance::resolve(None, None, false).expect("valid");
        assert_eq!(instance, Instance::Default);
        assert_eq!(instance.dir_name(), "ms-todo");
    }

    #[test]
    fn flag_beats_env_and_env_beats_build_kind() {
        let from_flag = Instance::resolve(Some("a"), Some("b"), true).expect("valid");
        assert_eq!(from_flag.dir_name(), "ms-todo-a");
        let from_env = Instance::resolve(None, Some("b"), false).expect("valid");
        assert_eq!(from_env.dir_name(), "ms-todo-b");
    }

    #[test]
    fn instance_names_cannot_escape_the_data_dir() {
        for bad in ["../x", "a/b", "a b", "."] {
            assert!(Instance::resolve(Some(bad), None, false).is_err(), "{bad}");
        }
    }

    #[test]
    fn detects_cargo_target_profiles() {
        assert!(path_has_cargo_target_profile_ancestor(Path::new(
            "/repo/target/debug/ms-todo"
        )));
        assert!(path_has_cargo_target_profile_ancestor(Path::new(
            "/repo/target/aarch64-apple-darwin/release/ms-todo"
        )));
        assert!(!path_has_cargo_target_profile_ancestor(Path::new(
            "/home/bk/.local/bin/ms-todo"
        )));
    }

    #[test]
    fn auth_dir_is_per_instance_and_config_is_shared() {
        let paths = Paths::under(
            Instance::Named("dev".into()),
            Path::new("/data"),
            Path::new("/config"),
        );
        assert_eq!(paths.auth_dir(), Path::new("/data/ms-todo-dev/auth"));
        assert_eq!(paths.config_file, Path::new("/config/ms-todo/config.toml"));
    }
}
