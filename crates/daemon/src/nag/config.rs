//! `[nag]` in config.toml (rung 9b):
//!
//! ```toml
//! [nag]
//! enabled = true                # the default
//! quiet_hours = "22:00-07:00"   # local; the default. "" for none
//! ```
//!
//! A bad setting keeps the default and says why in `doctor` and the
//! daemon's log, rather than stopping the daemon.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::NaiveTime;
use serde::Deserialize;

use super::schedule::QuietHours;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Config {
    pub enabled: bool,
    pub quiet_hours: Option<QuietHours>,
    /// Why a `[nag]` setting wasn't used, if one wasn't.
    pub problem: Option<String>,
    /// How long a nag's minute lasts: a minute, or shorter in a debug
    /// build with [`MINUTE_ENV`], so tests needn't wait.
    pub minute: Duration,
}

/// Debug builds only: a nag's minute in milliseconds, for tests.
const MINUTE_ENV: &str = "MS_TODO_NAG_MINUTE_MS";
/// Debug builds only: where notifications are written instead.
const NOTIFY_FILE_ENV: &str = "MS_TODO_NAG_NOTIFY_FILE";
/// Debug builds only: no notifier, whatever the system.
const NO_NOTIFIER_ENV: &str = "MS_TODO_NAG_NO_NOTIFIER";

const MINUTE: Duration = Duration::from_secs(60);

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            quiet_hours: Some(QuietHours {
                from: NaiveTime::from_hms_opt(22, 0, 0).unwrap_or(NaiveTime::MIN),
                until: NaiveTime::from_hms_opt(7, 0, 0).unwrap_or(NaiveTime::MIN),
            }),
            problem: None,
            minute: MINUTE,
        }
    }
}

/// Debug builds only: notifications go to this file, one line each,
/// instead of the screen ([`NOTIFY_FILE_ENV`]).
pub(crate) fn notify_file() -> Option<PathBuf> {
    debug_env(NOTIFY_FILE_ENV).map(PathBuf::from)
}

/// Debug builds only: behave as a system with no notifier, as Linux with
/// no D-Bus session bus is
/// ([`NO_NOTIFIER_ENV`]).
pub(crate) fn no_notifier() -> bool {
    debug_env(NO_NOTIFIER_ENV).is_some()
}

fn debug_env(name: &str) -> Option<String> {
    cfg!(debug_assertions)
        .then(|| std::env::var(name).ok())
        .flatten()
}

#[derive(Deserialize, Default)]
struct File {
    nag: Option<Section>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Section {
    enabled: Option<bool>,
    quiet_hours: Option<String>,
}

impl Config {
    pub fn load(config_file: &Path) -> Self {
        let minute = debug_env(MINUTE_ENV)
            .and_then(|ms| ms.parse::<u64>().ok())
            .filter(|ms| *ms > 0)
            .map_or(MINUTE, Duration::from_millis);
        Self {
            minute,
            ..Self::from_file(config_file)
        }
    }

    fn from_file(config_file: &Path) -> Self {
        let loaded = match std::fs::read_to_string(config_file) {
            Ok(raw) => Self::parse(&raw),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error.to_string()),
        };
        loaded.unwrap_or_else(|message| {
            let problem = format!(
                "{}: {message}; nagging with the defaults",
                config_file.display()
            );
            eprintln!("ms-todo daemon: {problem}");
            Self {
                problem: Some(problem),
                ..Self::default()
            }
        })
    }

    fn parse(raw: &str) -> Result<Self, String> {
        let file: File = toml::from_str(raw).map_err(|error| error.message().to_owned())?;
        let Some(section) = file.nag else {
            return Ok(Self::default());
        };
        let defaults = Self::default();
        let quiet_hours = match section.quiet_hours.as_deref().map(str::trim) {
            None => defaults.quiet_hours,
            Some("") => None,
            Some(text) => Some(QuietHours::parse(text)?),
        };
        Ok(Self {
            enabled: section.enabled.unwrap_or(defaults.enabled),
            quiet_hours,
            ..defaults
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_with_night_quiet_hours_unless_set() {
        let config = Config::parse("").expect("empty");
        assert!(config.enabled);
        assert_eq!(
            config.quiet_hours.map(QuietHours::label).as_deref(),
            Some("22:00-07:00")
        );
        let config = Config::parse("[nag]\nenabled = false\nquiet_hours = \"\"\n").expect("off");
        assert!(!config.enabled);
        assert_eq!(config.quiet_hours, None);
        let config = Config::parse("[nag]\nquiet_hours = \"23:30-06:00\"\n").expect("late");
        assert_eq!(
            config.quiet_hours.map(QuietHours::label).as_deref(),
            Some("23:30-06:00")
        );
    }

    #[test]
    fn a_bad_setting_is_refused_with_why() {
        let error = Config::parse("[nag]\nquiet_hours = \"late\"\n").expect_err("bad");
        assert!(error.contains("HH:MM-HH:MM"), "{error}");
        assert!(Config::parse("[nag]\nevery = 5\n").is_err());
    }
}
