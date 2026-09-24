//! Reading Graph's due dates. A due date is a date, not an instant (S11,
//! D-027): Graph stores it as midnight in whatever zone the writer used and
//! returns that instant in UTC, so BK's data holds 23:00Z, 00:00Z and 20:00Z
//! for "midnight". Converting to the local zone and rounding to the nearest
//! midnight gives the right day for all of them; truncating doesn't.

use chrono::{Duration, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};

/// How ms-todo writes a date, in flags, output and CSV.
pub const DATE_FORMAT: &str = "%Y-%m-%d";

/// The local date of a Graph `dateTimeTimeZone` due date.
pub fn local_due_date(date_time: &str, time_zone: &str) -> Option<NaiveDate> {
    let local = local_date_time(date_time, time_zone)?;
    Some((local + Duration::hours(12)).date())
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
    fn a_utc_value_near_midnight_rounds_rather_than_truncates() {
        // Whatever the local zone is, within 12 hours of UTC, a UTC midnight
        // is the same local day after rounding.
        assert_eq!(
            local_due_date("2026-09-26T00:00:00.0000000", "UTC"),
            Some(date("2026-09-26"))
        );
    }

    #[test]
    fn garbage_is_none() {
        assert_eq!(local_due_date("26 Sep", "UTC"), None);
    }
}
