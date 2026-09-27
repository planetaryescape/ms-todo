//! `[outbox]` in config.toml (D-065):
//!
//! ```toml
//! [outbox]
//! retention_days = 30        # the default: finished writes kept, and undoable, this long
//! unknown_lookup_hours = 24  # the default: how long an `unknown` write is looked for
//! ```
//!
//! A bad setting keeps the defaults and says why in `doctor` and the
//! daemon's log, rather than stopping the daemon.

use std::io::ErrorKind;
use std::path::Path;

use serde::Deserialize;

const DAY_SECS: i64 = 24 * 60 * 60;
const HOUR_SECS: i64 = 60 * 60;

/// At least a day: an `--idempotency-key` answers a repeat for 24 hours
/// after its command finished, from the command's operations.
const RETENTION_DAYS: std::ops::RangeInclusive<u32> = 1..=3650;
/// Up to 30 days: past that, a match would be too old to trust as ours.
const LOOKUP_HOURS: std::ops::RangeInclusive<u32> = 1..=720;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Config {
    /// How long a finished command is kept, and so how far back `undo`
    /// reaches.
    pub retention_days: u32,
    /// How long an `unknown` operation is looked for before it's flagged
    /// for the user.
    pub unknown_lookup_hours: u32,
    /// Why an `[outbox]` setting wasn't used, if one wasn't.
    pub problem: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            retention_days: 30,
            unknown_lookup_hours: u32::try_from(ms_todo_store::UNKNOWN_LOOKUP_SECS / HOUR_SECS)
                .unwrap_or(24),
            problem: None,
        }
    }
}

#[derive(Deserialize, Default)]
struct File {
    outbox: Option<Section>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Section {
    retention_days: Option<u32>,
    unknown_lookup_hours: Option<u32>,
}

impl Config {
    pub fn load(config_file: &Path) -> Self {
        let loaded = match std::fs::read_to_string(config_file) {
            Ok(raw) => Self::parse(&raw),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error.to_string()),
        };
        loaded.unwrap_or_else(|message| {
            let problem = format!(
                "{}: {message}; keeping the outbox's defaults",
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
        let defaults = Self::default();
        let Some(section) = file.outbox else {
            return Ok(defaults);
        };
        let retention_days = within(
            "retention_days",
            section.retention_days,
            &RETENTION_DAYS,
            defaults.retention_days,
        )?;
        let unknown_lookup_hours = within(
            "unknown_lookup_hours",
            section.unknown_lookup_hours,
            &LOOKUP_HOURS,
            defaults.unknown_lookup_hours,
        )?;
        Ok(Self {
            retention_days,
            unknown_lookup_hours,
            problem: None,
        })
    }

    pub fn retention_secs(&self) -> i64 {
        i64::from(self.retention_days) * DAY_SECS
    }

    pub fn unknown_lookup_secs(&self) -> i64 {
        i64::from(self.unknown_lookup_hours) * HOUR_SECS
    }
}

fn within(
    key: &str,
    value: Option<u32>,
    range: &std::ops::RangeInclusive<u32>,
    default: u32,
) -> Result<u32, String> {
    match value {
        None => Ok(default),
        Some(value) if range.contains(&value) => Ok(value),
        Some(value) => Err(format!(
            "[outbox] {key} = {value} is out of range; it takes {} to {}",
            range.start(),
            range.end()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thirty_days_and_a_day_unless_set() {
        let config = Config::parse("").expect("empty");
        assert_eq!(
            (config.retention_days, config.unknown_lookup_hours),
            (30, 24)
        );
        assert_eq!(config.unknown_lookup_secs(), 24 * 60 * 60);
        let config = Config::parse("[outbox]\nretention_days = 7\nunknown_lookup_hours = 48\n")
            .expect("set");
        assert_eq!(
            (config.retention_days, config.unknown_lookup_hours),
            (7, 48)
        );
        assert_eq!(config.retention_secs(), 7 * 24 * 60 * 60);
    }

    #[test]
    fn a_bad_setting_is_refused_with_why() {
        let error = Config::parse("[outbox]\nretention_days = 0\n").expect_err("zero");
        assert!(error.contains("1 to 3650"), "{error}");
        let error = Config::parse("[outbox]\nunknown_lookup_hours = 1000\n").expect_err("long");
        assert!(error.contains("1 to 720"), "{error}");
        assert!(Config::parse("[outbox]\nretention = 5\n").is_err());
        assert!(Config::parse("[outbox]\nretention_days = -1\n").is_err());
    }
}
