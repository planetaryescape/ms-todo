// Adapted from spotuify crates/spotuify-protocol/src/paths.rs @ d807e5e4f9d2f09878cdc22309af3589623f7785
// (instance naming and the Cargo `target/` detection only).

//! Where ms-todo keeps its files (docs/blueprint/01-architecture.md#files).
//!
//! Dev and installed builds are kept apart by an instance name, so an agent
//! running a local build never touches the installed binary's sign-in:
//!
//! - `MS_TODO_INSTANCE=<name>` (or `--instance <name>`) uses `ms-todo-<name>`,
//!   except `default`, which picks the installed instance from any build.
//! - A binary run from Cargo's `target/` tree, or any debug build, defaults to
//!   the `dev` instance, so its data lives in `ms-todo-dev`.
//! - Everything else uses plain `ms-todo`.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

pub const APP_NAME: &str = "ms-todo";
pub const INSTANCE_ENV: &str = "MS_TODO_INSTANCE";
/// Overrides where `config.toml` lives.
pub const CONFIG_DIR_ENV: &str = "MS_TODO_CONFIG_DIR";
const DEV_INSTANCE: &str = "dev";
/// The name that selects [`Instance::Default`], matching its label.
const DEFAULT_INSTANCE: &str = "default";

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
            // Lets a target/ build reach the installed instance, and lets the
            // daemon be spawned with the exact instance its client resolved.
            Some(DEFAULT_INSTANCE) => Ok(Self::Default),
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
    /// Passing it back to [`Instance::resolve`] gives the same instance.
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
    /// `config.toml` in `$MS_TODO_CONFIG_DIR`, `$XDG_CONFIG_HOME/ms-todo` or
    /// `~/.config/ms-todo`, on macOS as well as Linux (D-035). Not per
    /// instance: it only holds settings that are the same for every build,
    /// such as `auth.client_id`.
    pub config_file: PathBuf,
    /// Kept 0700; holds the daemon's socket (0600), pid file and lock.
    /// `<runtime_dir>/ms-todo[-<instance>]` where the platform has a runtime
    /// directory (Linux), otherwise `<data_dir>/ms-todo[-<instance>]/run`.
    pub run_dir: PathBuf,
}

impl Paths {
    pub fn resolve(instance: Instance) -> Result<Self, PathsError> {
        let data_base = dirs::data_dir().ok_or(PathsError::NoPlatformDir("data"))?;
        let config_dir = config_dir_from(
            std::env::var_os(CONFIG_DIR_ENV).as_deref().map(Path::new),
            std::env::var_os("XDG_CONFIG_HOME")
                .as_deref()
                .map(Path::new),
            dirs::home_dir().as_deref(),
        )
        .ok_or(PathsError::NoPlatformDir("config"))?;
        let runtime_base = dirs::runtime_dir();
        Ok(Self::under(
            instance,
            &data_base,
            &config_dir,
            runtime_base.as_deref(),
        ))
    }

    /// `config_dir` is ms-todo's own config directory, already resolved.
    pub fn under(
        instance: Instance,
        data_base: &Path,
        config_dir: &Path,
        runtime_base: Option<&Path>,
    ) -> Self {
        let data_dir = data_base.join(instance.dir_name());
        let run_dir = match runtime_base {
            Some(runtime) => runtime.join(instance.dir_name()),
            None => data_dir.join("run"),
        };
        Self {
            config_file: config_dir.join("config.toml"),
            data_dir,
            run_dir,
            instance,
        }
    }

    /// Kept 0700; holds `token.json` (0600) and its lock file.
    pub fn auth_dir(&self) -> PathBuf {
        self.data_dir.join("auth")
    }

    /// The daemon's Unix socket.
    pub fn socket_path(&self) -> PathBuf {
        self.run_dir.join("daemon.sock")
    }

    /// Holds the running daemon's PID, written by the daemon itself.
    pub fn pid_file(&self) -> PathBuf {
        self.run_dir.join("daemon.pid")
    }

    /// The running daemon holds an exclusive lock on this file for its whole
    /// life, so "is a daemon running?" never depends on a reused PID.
    pub fn daemon_lock_file(&self) -> PathBuf {
        self.run_dir.join("daemon.lock")
    }

    /// The cache: SQLite in WAL mode, owned by the daemon.
    pub fn database_file(&self) -> PathBuf {
        self.data_dir.join("ms-todo.db")
    }

    /// The daemon's stderr.
    pub fn daemon_log_file(&self) -> PathBuf {
        self.data_dir.join("logs").join("daemon.log")
    }
}

/// ms-todo's config directory: `$MS_TODO_CONFIG_DIR`, then
/// `$XDG_CONFIG_HOME/ms-todo`, then `~/.config/ms-todo`. Deliberately not
/// `dirs::config_dir()`, which is `~/Library/Application Support` on macOS;
/// CLIs keep config in `~/.config` there too (D-035). Empty or relative XDG
/// values are ignored, as the XDG spec says.
pub fn config_dir_from(
    override_dir: Option<&Path>,
    xdg_config_home: Option<&Path>,
    home: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(dir) = override_dir.filter(|dir| !dir.as_os_str().is_empty()) {
        return Some(dir.to_path_buf());
    }
    let base = match xdg_config_home.filter(|dir| dir.is_absolute()) {
        Some(xdg) => xdg.to_path_buf(),
        None => home?.join(".config"),
    };
    Some(base.join(APP_NAME))
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
            Path::new("/config/ms-todo"),
            None,
        );
        assert_eq!(paths.auth_dir(), Path::new("/data/ms-todo-dev/auth"));
        assert_eq!(paths.config_file, Path::new("/config/ms-todo/config.toml"));
    }

    #[test]
    fn config_dir_prefers_the_override_then_xdg_then_dot_config() {
        let home = Some(Path::new("/home/bk"));
        let xdg = Some(Path::new("/xdg"));
        assert_eq!(
            config_dir_from(Some(Path::new("/custom")), xdg, home),
            Some(PathBuf::from("/custom"))
        );
        assert_eq!(
            config_dir_from(None, xdg, home),
            Some(PathBuf::from("/xdg/ms-todo"))
        );
        assert_eq!(
            config_dir_from(None, None, home),
            Some(PathBuf::from("/home/bk/.config/ms-todo"))
        );
    }

    #[test]
    fn empty_or_relative_config_values_are_ignored() {
        let home = Some(Path::new("/home/bk"));
        let dot_config = Some(PathBuf::from("/home/bk/.config/ms-todo"));
        assert_eq!(
            config_dir_from(Some(Path::new("")), Some(Path::new("")), home),
            dot_config
        );
        assert_eq!(
            config_dir_from(None, Some(Path::new("relative")), home),
            dot_config
        );
        assert_eq!(config_dir_from(None, None, None), None);
    }

    #[test]
    fn default_selects_the_installed_instance_from_any_build() {
        let instance = Instance::resolve(Some("default"), None, true).expect("valid");
        assert_eq!(instance, Instance::Default);
        let round_trip = Instance::resolve(Some(instance.label()), None, true).expect("valid");
        assert_eq!(round_trip, instance);
    }

    #[test]
    fn the_socket_is_per_instance_under_the_runtime_dir_or_the_data_dir() {
        let linux = Paths::under(
            Instance::Named("dev".into()),
            Path::new("/data"),
            Path::new("/config/ms-todo"),
            Some(Path::new("/run/user/1000")),
        );
        assert_eq!(
            linux.socket_path(),
            Path::new("/run/user/1000/ms-todo-dev/daemon.sock")
        );
        let macos = Paths::under(
            Instance::Default,
            Path::new("/data"),
            Path::new("/config/ms-todo"),
            None,
        );
        assert_eq!(
            macos.socket_path(),
            Path::new("/data/ms-todo/run/daemon.sock")
        );
        assert_eq!(macos.pid_file(), Path::new("/data/ms-todo/run/daemon.pid"));
    }
}
