//! A recurrence as a whole string, for `--recur` on `tasks add` and
//! `tasks edit` (rung 8e): the same `every …` grammar quick add reads
//! inside a title, but the input must be nothing else.

use chrono::NaiveDate;

use crate::dates::{NotUnderstood, ParseContext};
use crate::recurrence::{self, Recurrence, RecurrenceEnd};

/// Read all of `input` as a recurrence: `every mon`, `every 2 weeks on
/// tue, thu`, `every month on the 1st until dec`, `daily`. The leading
/// `every` may be left out (`weekday`, `last fri`). Its first occurrence
/// is the first day the pattern falls on from `anchor` (a due date the
/// caller also sets), else from today. A time is refused: it
/// belongs in the reminder.
pub fn read_recurrence(
    input: &str,
    ctx: &ParseContext,
    anchor: Option<NaiveDate>,
) -> Result<Recurrence, NotUnderstood> {
    let text = input
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    if text.is_empty() || text == "-" {
        return Err(NotUnderstood(
            "a recurrence is wanted here, such as \"every mon\" or \"every month on the 1st\""
                .into(),
        ));
    }
    let read = whole(&text, ctx)
        .or_else(|| (!text.starts_with("every ")).then(|| whole(&format!("every {text}"), ctx))?)
        .ok_or_else(|| NotUnderstood(format!("didn't understand \"{text}\" as a recurrence")))?;
    if read.time.is_some() {
        return Err(NotUnderstood(
            "a recurrence has no time of day here; put the time in --reminder".into(),
        ));
    }
    let start = Recurrence::first_on_or_after(&read.pattern, anchor.unwrap_or_else(|| ctx.today()));
    if let RecurrenceEnd::Until(until) = read.end
        && until < start
    {
        return Err(NotUnderstood(format!(
            "it would end on {until} but its first time is {start}"
        )));
    }
    Ok(Recurrence {
        pattern: read.pattern,
        end: read.end,
        start,
    })
}

/// A recurrence that is all of `text`.
fn whole(text: &str, ctx: &ParseContext) -> Option<recurrence::Read> {
    recurrence::read_at(text, 0, ctx).filter(|read| read.until == text.len())
}

#[cfg(test)]
mod tests {
    use chrono::{FixedOffset, TimeZone};
    use serde_json::json;

    use super::*;

    /// Thursday 24 September 2026, 10:00 London (UTC+1).
    fn ctx() -> ParseContext {
        let offset = FixedOffset::east_opt(3600).expect("offset");
        ParseContext::new(
            offset
                .with_ymd_and_hms(2026, 9, 24, 10, 0, 0)
                .single()
                .expect("time"),
        )
    }

    fn day(text: &str) -> NaiveDate {
        NaiveDate::parse_from_str(text, "%Y-%m-%d").expect("day")
    }

    #[test]
    fn a_weekly_recurrence_starts_on_its_next_day_from_today() {
        let read = read_recurrence("every Mon", &ctx(), None).expect("read");
        assert_eq!(read.start, day("2026-09-28"));
        assert_eq!(read.to_graph()["pattern"]["daysOfWeek"], json!(["monday"]));
        assert_eq!(read.to_graph()["range"]["startDate"], "2026-09-28");
    }

    #[test]
    fn every_may_be_left_out_and_an_anchor_is_the_start() {
        let read =
            read_recurrence("month on the 1st", &ctx(), Some(day("2026-10-01"))).expect("read");
        assert_eq!(read.start, day("2026-10-01"));
        assert_eq!(read.to_graph()["pattern"]["type"], "absoluteMonthly");
        assert_eq!(
            read_recurrence("daily", &ctx(), None).expect("daily").start,
            day("2026-09-24")
        );
    }

    #[test]
    fn anything_but_a_recurrence_is_refused() {
        for text in ["", "-", "every", "tomorrow", "every mon and then some"] {
            assert!(read_recurrence(text, &ctx(), None).is_err(), "{text:?}");
        }
        let timed = read_recurrence("every day at 9am", &ctx(), None).expect_err("time");
        assert!(timed.0.contains("--reminder"), "{}", timed.0);
        let ended = read_recurrence("every day until 1 sep 2026", &ctx(), None).expect_err("end");
        assert!(ended.0.contains("end"), "{}", ended.0);
    }
}
