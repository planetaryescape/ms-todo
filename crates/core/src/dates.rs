//! Reading Graph's due dates. A due date is a date, not an instant (S11,
//! D-027): Graph stores it as midnight in whatever zone the writer used and
//! returns that instant in UTC, so BK's data holds 23:00Z, 00:00Z and 20:00Z
//! for "midnight". The writer's zone is lost (D-059), so dates written across
//! widely separated zones cannot always be recovered exactly.

use chrono::{Datelike, Duration, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};

/// How ms-todo writes a date, in flags, output and CSV.
pub const DATE_FORMAT: &str = "%Y-%m-%d";

/// How a reminder travels from a client to the daemon: local time.
pub const REMINDER_FORMAT: &str = "%Y-%m-%dT%H:%M";

/// The local date of a Graph `dateTimeTimeZone` due date.
pub fn local_due_date(date_time: &str, time_zone: &str) -> Option<NaiveDate> {
    local_due_date_in(date_time, time_zone, &Local)
}

fn local_due_date_in<Tz: TimeZone>(
    date_time: &str,
    time_zone: &str,
    local: &Tz,
) -> Option<NaiveDate> {
    let naive = parse_graph_date_time(date_time)?;
    if time_zone.eq_ignore_ascii_case("UTC") {
        let local_time = Utc.from_utc_datetime(&naive).with_timezone(local);
        if local_time.time() == chrono::NaiveTime::MIN {
            return Some(local_time.date_naive());
        }
    }
    naive
        .checked_add_signed(Duration::hours(12))
        .map(|at| at.date())
}

/// The day a task was completed. Graph records `completedDateTime` as a
/// UTC date, midnight UTC with no time of day (S12), not as the user's
/// local midnight, so that value is its UTC calendar date: rounding it in
/// the local zone would move it a day in Auckland (+13). A value with a
/// real time is converted to the local zone and its date taken, with no
/// rounding.
pub fn completion_date(date_time: &str, time_zone: &str) -> Option<NaiveDate> {
    completion_date_in(date_time, time_zone, &Local)
}

fn completion_date_in<Tz: TimeZone>(
    date_time: &str,
    time_zone: &str,
    local: &Tz,
) -> Option<NaiveDate> {
    let naive = parse_graph_date_time(date_time)?;
    if !time_zone.eq_ignore_ascii_case("UTC") {
        return Some(naive.date());
    }
    if naive.time() == chrono::NaiveTime::MIN {
        return Some(naive.date());
    }
    Some(
        Utc.from_utc_datetime(&naive)
            .with_timezone(local)
            .date_naive(),
    )
}

/// A day as the heading of what was done on it: `Today`, `Yesterday`,
/// else `Mon 21 Sep`, with the year when it isn't this year.
pub fn day_heading(day: NaiveDate, today: NaiveDate) -> String {
    match today.signed_duration_since(day).num_days() {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        _ if day.year() == today.year() => day.format("%a %-d %b").to_string(),
        _ => day.format("%a %-d %b %Y").to_string(),
    }
}

/// The heading of what was completed on `day`: [`day_heading`], or for a
/// completion Microsoft To Do hasn't answered yet, which has no day,
/// "Not synced yet".
pub fn completion_heading(day: Option<NaiveDate>, today: NaiveDate) -> String {
    day.map_or_else(
        || "Not synced yet".to_owned(),
        |day| day_heading(day, today),
    )
}

/// A Graph `dateTimeTimeZone` in the local zone. Graph returns UTC unless a
/// request asks for another zone with `Prefer: outlook.timezone`, and a
/// value in another zone is taken as it is.
pub fn local_date_time(date_time: &str, time_zone: &str) -> Option<NaiveDateTime> {
    let naive = parse_graph_date_time(date_time)?;
    Some(if time_zone.eq_ignore_ascii_case("UTC") {
        Utc.from_utc_datetime(&naive)
            .with_timezone(&Local)
            .naive_local()
    } else {
        naive
    })
}

/// Graph writes `2026-09-25T23:00:00.0000000`: no offset, up to seven
/// fractional digits.
pub fn parse_graph_date_time(value: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f").ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(value: &str) -> NaiveDate {
        NaiveDate::parse_from_str(value, "%Y-%m-%d").expect("date")
    }

    #[test]
    fn midnight_written_in_the_values_own_zone_rounds_to_that_day() {
        assert_eq!(
            local_due_date("2026-09-26T00:00:00.0000000", "Europe/London"),
            Some(date("2026-09-26"))
        );
        // S11k read back in London: 05:00 is still the 26th.
        assert_eq!(
            local_due_date("2026-09-26T05:00:00.0000000", "Europe/London"),
            Some(date("2026-09-26"))
        );
    }

    #[test]
    fn utc_due_dates_keep_the_day_in_eastern_reader_zones() {
        let zone = |hours: i32| chrono::FixedOffset::east_opt(hours * 3600).expect("offset");
        for hours in [13, 14] {
            // Midnight UTC, London and Dubai as returned by Graph all mean 26 Sep.
            for time in ["00:00", "23:00", "20:00"] {
                let day = if time == "00:00" { "26" } else { "25" };
                let value = format!("2026-09-{day}T{time}:00.0000000");
                assert_eq!(
                    local_due_date_in(&value, "UTC", &zone(hours)),
                    Some(date("2026-09-26")),
                    "reader {hours:+}, UTC time {time}"
                );
            }
            // UTC+13/+14 writers also mean 26 Sep. Their UTC instant is
            // exactly midnight in the matching reader zone.
            let utc_hour = 24 - hours;
            let value = format!("2026-09-25T{utc_hour:02}:00:00.0000000");
            assert_eq!(
                local_due_date_in(&value, "UTC", &zone(hours)),
                Some(date("2026-09-26")),
                "reader and writer {hours:+}"
            );
        }
        assert_eq!(
            local_due_date_in("2026-09-26T00:00:00.0000000", "UTC", &zone(-5)),
            Some(date("2026-09-26"))
        );
    }

    #[test]
    fn non_utc_due_value_is_wall_time() {
        assert_eq!(
            local_due_date_in(
                "2026-09-26T13:00:00.0000000",
                "Pacific/Kiritimati",
                &chrono::FixedOffset::west_opt(5 * 3600).expect("offset"),
            ),
            Some(date("2026-09-27"))
        );
    }

    #[test]
    fn a_utc_midnight_completion_is_its_utc_date_in_any_zone() {
        let zone = |hours: i32| chrono::FixedOffset::east_opt(hours * 3600).expect("offset");
        let midnight = "2026-12-10T00:00:00.0000000";
        // Auckland in summer (NZDT, +13), London in summer (BST, +1), and
        // New York (EST, -5): Graph's UTC date, never rounded to a
        // neighbour.
        for hours in [13, 1, -5] {
            assert_eq!(
                completion_date_in(midnight, "UTC", &zone(hours)),
                Some(date("2026-12-10")),
                "{hours:+}"
            );
        }
        // A real time is converted and its local date taken.
        let timed = "2026-12-10T12:30:00.0000000";
        assert_eq!(
            completion_date_in(timed, "UTC", &zone(13)),
            Some(date("2026-12-11"))
        );
        assert_eq!(
            completion_date_in(timed, "UTC", &zone(-5)),
            Some(date("2026-12-10"))
        );
        assert_eq!(
            completion_date_in("2026-12-10T03:00:00.0000000", "UTC", &zone(-5)),
            Some(date("2026-12-09"))
        );
        assert_eq!(completion_date("junk", "UTC"), None);
    }

    #[test]
    fn completion_headings_are_relative_to_today() {
        let today = date("2026-09-24");
        assert_eq!(day_heading(today, today), "Today");
        assert_eq!(day_heading(date("2026-09-23"), today), "Yesterday");
        assert_eq!(day_heading(date("2026-09-21"), today), "Mon 21 Sep");
        assert_eq!(day_heading(date("2025-12-31"), today), "Wed 31 Dec 2025");
        assert_eq!(completion_heading(None, today), "Not synced yet");
    }

    #[test]
    fn garbage_is_none() {
        assert_eq!(local_due_date("26 Sep", "UTC"), None);
    }
}
