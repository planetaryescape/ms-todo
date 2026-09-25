//! `[my_day]` in config.toml (D-024, D-035): when a day's My Day ends.
//!
//! ```toml
//! [my_day]
//! rollover_time = "04:00"   # local; the default is "00:00"
//! ```
//!
//! A bad setting keeps the default and says why in `doctor` and the
//! daemon's log, rather than stopping the daemon.

use std::io::ErrorKind;
use std::path::Path;

use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use serde::Deserialize;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Config {
    /// When a day's My Day ends, local time.
    pub rollover_time: NaiveTime,
    /// Why `my_day.rollover_time` wasn't used, if it wasn't.
    pub problem: Option<String>,
    /// My Day's day, fixed by [`TODAY_ENV`] in a debug build, so tests
    /// don't depend on the clock.
    pub today: Option<NaiveDate>,
}

/// Debug builds only: My Day's day as `YYYY-MM-DD`, for tests.
const TODAY_ENV: &str = "MS_TODO_MY_DAY_TODAY";

impl Default for Config {
    fn default() -> Self {
        Self {
            rollover_time: NaiveTime::MIN,
            problem: None,
            today: None,
        }
    }
}

#[derive(Deserialize, Default)]
struct File {
    my_day: Option<Section>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Section {
    rollover_time: Option<String>,
}

impl Config {
    pub fn load(config_file: &Path) -> Self {
        let today = cfg!(debug_assertions)
            .then(|| std::env::var(TODAY_ENV).ok())
            .flatten()
            .and_then(|day| NaiveDate::parse_from_str(&day, "%Y-%m-%d").ok());
        Self {
            today,
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
            let problem = format!("{}: {message}; using 00:00", config_file.display());
            eprintln!("ms-todo daemon: {problem}");
            Self {
                problem: Some(problem),
                ..Self::default()
            }
        })
    }

    fn parse(raw: &str) -> Result<Self, String> {
        let file: File = toml::from_str(raw).map_err(|error| error.message().to_owned())?;
        let Some(time) = file.my_day.and_then(|section| section.rollover_time) else {
            return Ok(Self::default());
        };
        let rollover_time = NaiveTime::parse_from_str(time.trim(), "%H:%M").map_err(|_| {
            format!(
                "my_day.rollover_time = {time:?} isn't a time: use \"HH:MM\", such as \"04:00\""
            )
        })?;
        Ok(Self {
            rollover_time,
            ..Self::default()
        })
    }

    /// My Day's day at local time `now`: today, or yesterday before the
    /// rollover time, so work after midnight with `rollover_time = "04:00"`
    /// still counts as the day before.
    pub fn day_at(&self, now: NaiveDateTime) -> NaiveDate {
        let since_midnight = self.rollover_time - NaiveTime::MIN;
        (now - since_midnight).date()
    }

    /// My Day's day now.
    pub fn today(&self) -> NaiveDate {
        self.today
            .unwrap_or_else(|| self.day_at(chrono::Local::now().naive_local()))
    }

    /// `HH:MM`, for `doctor`.
    pub fn rollover_label(&self) -> String {
        self.rollover_time.format("%H:%M").to_string()
    }
}

/// Whether the rollover is due on `today`, given the day it last ran for:
/// once a day, and once after days off.
pub(crate) fn rollover_due(last: Option<NaiveDate>, today: NaiveDate) -> bool {
    last.is_none_or(|last| last < today)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(value: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M").expect("time")
    }

    fn day(value: &str) -> NaiveDate {
        NaiveDate::parse_from_str(value, "%Y-%m-%d").expect("day")
    }

    #[test]
    fn the_default_is_midnight_and_a_setting_is_read() {
        assert_eq!(Config::parse("").expect("empty"), Config::default());
        let config = Config::parse("[my_day]\nrollover_time = \"04:00\"\n").expect("parsed");
        assert_eq!(config.rollover_label(), "04:00");
        assert!(Config::parse("[my_day]\nrollover_time = \"4pm\"\n").is_err());
        assert!(Config::parse("[my_day]\nrolover_time = \"04:00\"\n").is_err());
        // Other sections are someone else's.
        assert!(Config::parse("[tui]\ntheme = \"nord\"\n").is_ok());
    }

    #[test]
    fn my_days_day_turns_over_at_the_rollover_time() {
        let midnight = Config::default();
        assert_eq!(midnight.day_at(at("2026-09-25 00:00")), day("2026-09-25"));
        assert_eq!(midnight.day_at(at("2026-09-24 23:59")), day("2026-09-24"));
        let late = Config::parse("[my_day]\nrollover_time = \"04:00\"\n").expect("parsed");
        assert_eq!(late.day_at(at("2026-09-25 03:59")), day("2026-09-24"));
        assert_eq!(late.day_at(at("2026-09-25 04:00")), day("2026-09-25"));
    }

    #[test]
    fn the_rollover_runs_once_a_day_and_once_after_days_off() {
        assert!(rollover_due(None, day("2026-09-25")));
        assert!(rollover_due(Some(day("2026-09-21")), day("2026-09-25")));
        assert!(!rollover_due(Some(day("2026-09-25")), day("2026-09-25")));
    }
}
