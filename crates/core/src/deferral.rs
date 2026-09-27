//! Defer and Someday (rung 9a): when a task comes back into view. Both
//! live in ms-todo's own extension, since Graph's `startDateTime` moves
//! the due date with it (S11, D-058) and can't be a Things-style "when".
//! The due date stays the deadline the phone shows.
//!
//! One rule, shared by the daemon and the TUI: an open task is hidden
//! from the everyday views while it's Someday, or deferred to a day after
//! today. On its day it simply shows again; nothing is written.

use chrono::NaiveDate;
use serde_json::Value;

use crate::DATE_FORMAT;

/// The day a task comes back, `YYYY-MM-DD` (a local day), in our
/// extension.
pub const DEFER_UNTIL: &str = "deferUntil";
/// `true` while a task is parked as Someday, in our extension.
pub const SOMEDAY: &str = "someday";

/// A task's defer day, read from our extension.
pub fn defer_until(extension: Option<&Value>) -> Option<NaiveDate> {
    extension?
        .get(DEFER_UNTIL)?
        .as_str()
        .and_then(|day| NaiveDate::parse_from_str(day, DATE_FORMAT).ok())
}

/// Whether a task is Someday, read from our extension.
pub fn someday(extension: Option<&Value>) -> bool {
    extension
        .and_then(|extension| extension.get(SOMEDAY))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Whether an open task with these fields is out of view on `today`
/// (local). A completed task never is: it's done, whenever it was for.
pub fn hidden(
    defer_until: Option<NaiveDate>,
    someday: bool,
    completed: bool,
    today: NaiveDate,
) -> bool {
    !completed && (someday || defer_until.is_some_and(|day| day > today))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn day(value: &str) -> NaiveDate {
        NaiveDate::parse_from_str(value, DATE_FORMAT).expect("day")
    }

    #[test]
    fn a_task_shows_again_on_its_day_and_someday_waits_for_the_user() {
        let today = day("2026-10-01");
        assert!(hidden(Some(day("2026-10-02")), false, false, today));
        assert!(!hidden(Some(today), false, false, today), "its day");
        assert!(!hidden(Some(day("2026-09-30")), false, false, today));
        assert!(hidden(None, true, false, today));
        assert!(hidden(Some(day("2026-09-30")), true, false, today));
        assert!(!hidden(Some(day("2026-10-02")), true, true, today), "done");
        assert!(!hidden(None, false, false, today));
    }

    #[test]
    fn fields_are_read_from_our_extension() {
        let extension = json!({ "deferUntil": "2026-10-02", "someday": true });
        assert_eq!(defer_until(Some(&extension)), Some(day("2026-10-02")));
        assert!(someday(Some(&extension)));
        let odd = json!({ "deferUntil": "soon", "someday": "yes" });
        assert_eq!(defer_until(Some(&odd)), None);
        assert!(!someday(Some(&odd)));
        assert_eq!(defer_until(None), None);
    }
}
