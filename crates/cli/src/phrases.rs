//! `--due`, `--reminder` and `--importance` read through `ms-todo-nlp`,
//! so the flags take what the TUI's fields take: `tomorrow`, `fri 17:30`,
//! `+2w`, `p1`. Each is a clap value parser, so a value that can't be
//! read fails as a usage error (exit 2) naming what wasn't understood,
//! and the daemon only ever sees the canonical forms.

use chrono::Local;
use ms_todo_core::{DATE_FORMAT, REMINDER_FORMAT};
use ms_todo_nlp::{
    NotUnderstood, ParseContext, Reading, read_due, read_importance, read_past_date, read_reminder,
};
use ms_todo_protocol::{Clearable, Importance};

pub(crate) fn now() -> ParseContext {
    ParseContext::new(Local::now().fixed_offset())
}

/// `--due`: a date, or empty or `-` to clear it.
pub fn due(value: &str) -> Result<Clearable<String>, NotUnderstood> {
    Ok(match read_due(value, &now())? {
        Reading::Clear => Clearable::Clear,
        Reading::Set { value, .. } => Clearable::Set(value.format(DATE_FORMAT).to_string()),
    })
}

/// A day that must be given: `--to` and `--due-before`, read ahead as
/// `--due` is.
pub fn day(value: &str) -> Result<String, NotUnderstood> {
    match due(value)? {
        Clearable::Set(day) => Ok(day),
        Clearable::Clear => Err(NotUnderstood(
            "a day is wanted here, such as today, fri or 2026-10-02".into(),
        )),
    }
}

/// `--since` and `--until`: a day read looking back, so `mon` is the
/// latest Monday and `12 sep` the latest 12 September.
pub fn past_day(value: &str) -> Result<String, NotUnderstood> {
    read_past_date(value, &now()).map(|day| day.format(DATE_FORMAT).to_string())
}

/// The local day `days` from today (negative: before), as `YYYY-MM-DD`:
/// `--overdue` is due before today, and `done` starts 7 days ago.
pub fn days_from_today(days: i64) -> String {
    (Local::now().date_naive() + chrono::Duration::days(days))
        .format(DATE_FORMAT)
        .to_string()
}

/// `--reminder`: a date and time, or a time alone, or empty or `-` to
/// turn it off.
pub fn reminder(value: &str) -> Result<Clearable<String>, NotUnderstood> {
    Ok(match read_reminder(value, &now())? {
        Reading::Clear => Clearable::Clear,
        Reading::Set { value, .. } => Clearable::Set(value.format(REMINDER_FORMAT).to_string()),
    })
}

/// `--importance`: `1`–`4`, `p1`–`p4`, or `high`, `normal` or `low`.
pub fn importance(value: &str) -> Result<Importance, NotUnderstood> {
    read_importance(value).map(protocol_importance)
}

/// A level as `ms-todo-nlp` reads it, as the protocol carries it.
pub(crate) fn protocol_importance(level: ms_todo_nlp::Importance) -> Importance {
    match level {
        ms_todo_nlp::Importance::High => Importance::High,
        ms_todo_nlp::Importance::Normal => Importance::Normal,
        ms_todo_nlp::Importance::Low => Importance::Low,
    }
}

/// A set value, or nothing when the flag cleared it: a new task has
/// nothing to clear.
pub fn set_only(value: Option<Clearable<String>>) -> Option<String> {
    match value {
        Some(Clearable::Set(value)) => Some(value),
        _ => None,
    }
}
