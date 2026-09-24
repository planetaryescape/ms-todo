//! The rule table: one regex per phrase shape, and what it resolves to
//! (docs/blueprint/06-natural-language.md, D-026). The patterns hold no
//! anchors: whole-string mode anchors them to the start of what's left,
//! and rung 6's scanner will run the same patterns inside a title to find
//! spans, adding its masking passes and guards (`Friday's`) around them.
//!
//! Patterns see lowercased text with single spaces.

use std::sync::LazyLock;

use chrono::{Datelike, Days, Months, NaiveDate, NaiveDateTime, NaiveTime, Weekday};
use regex::{Captures, Regex};

use super::ParseContext;
use super::words::{MONTHS, RELATIVE_DAYS, WEEKDAYS, alternation, lookup, number};

/// What one rule reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Value {
    Date(NaiveDate),
    Time(NaiveTime),
    DateTime(NaiveDateTime),
}

pub(super) struct Rule {
    /// Anchored at the start, and ending on a word boundary.
    pub regex: Regex,
    /// `None` when the text has the shape but isn't a real date or time,
    /// such as `31/02`.
    pub resolve: fn(&Captures, &ParseContext) -> Option<Value>,
}

/// Phrases that name a day, or a day and a time in one token.
pub(super) static DATE_RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    let weekday = alternation(WEEKDAYS);
    let month = alternation(MONTHS);
    let count = format!(r"\d+|{}", alternation(super::words::NUMBERS));
    let unit = r"days?|weeks?|months?";
    let ordinal = r"(?:st|nd|rd|th)?";
    vec![
        rule(
            r"(\d{4})-(\d{1,2})-(\d{1,2})t(\d{1,2}):(\d{2})",
            iso_date_time,
        ),
        rule(r"(\d{4})-(\d{1,2})-(\d{1,2})", iso_date),
        rule(&format!("({})", alternation(RELATIVE_DAYS)), relative_day),
        rule(&format!("this ({weekday})"), this_weekday),
        rule(&format!("next ({weekday})"), next_weekday),
        rule(&format!("({weekday})"), bare_weekday),
        rule(
            "next week|next month|end of week|eow|end of month|eom",
            period,
        ),
        rule(&format!("in ({count}) ({unit})"), ahead),
        rule(&format!("({count}) ({unit}) from (?:today|now)"), ahead),
        rule(&format!("({count}) ({unit}) ago"), ago),
        rule(&format!(r"([+-])(\d+) ?({unit}|d|w|m)"), signed),
        rule(
            &format!(r"(\d{{1,2}}){ordinal} ({month})(?:,? (\d{{4}}))?"),
            day_month,
        ),
        rule(
            &format!(r"({month}) (\d{{1,2}}){ordinal}(?:,? (\d{{4}}))?"),
            month_day,
        ),
        rule(r"(\d{1,2})/(\d{1,2})", day_slash_month),
    ]
});

/// Phrases that name a time of day.
pub(super) static TIME_RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    vec![
        rule(r"(\d{1,2})(?:[:.](\d{2}))? ?(am|pm)", twelve_hour),
        rule(r"(\d{1,2}):(\d{2})", twenty_four_hour),
        rule("noon|midday|midnight", named_time),
    ]
});

fn rule(pattern: &str, resolve: fn(&Captures, &ParseContext) -> Option<Value>) -> Rule {
    // The patterns are constants, so a bad one fails every test.
    let regex = Regex::new(&format!(r"^(?:{pattern})\b")).expect("a valid date rule");
    Rule { regex, resolve }
}

fn text<'h>(captures: &Captures<'h>, group: usize) -> &'h str {
    captures.get(group).map_or("", |found| found.as_str())
}

fn int(captures: &Captures, group: usize) -> Option<u32> {
    text(captures, group).parse().ok()
}

fn iso_date(captures: &Captures, _: &ParseContext) -> Option<Value> {
    let year = text(captures, 1).parse().ok()?;
    NaiveDate::from_ymd_opt(year, int(captures, 2)?, int(captures, 3)?).map(Value::Date)
}

/// The form the CLI's `--reminder` took before phrases: `2026-09-26T09:30`.
fn iso_date_time(captures: &Captures, ctx: &ParseContext) -> Option<Value> {
    let Value::Date(date) = iso_date(captures, ctx)? else {
        return None;
    };
    let time = NaiveTime::from_hms_opt(int(captures, 4)?, int(captures, 5)?, 0)?;
    Some(Value::DateTime(date.and_time(time)))
}

fn relative_day(captures: &Captures, ctx: &ParseContext) -> Option<Value> {
    let days = lookup(RELATIVE_DAYS, text(captures, 1))?;
    add_days(ctx.today(), days).map(Value::Date)
}

fn weekday(captures: &Captures) -> Option<Weekday> {
    lookup(WEEKDAYS, text(captures, 1))
}

/// Days from `from` to the next `target`, 0 when `from` is one.
fn days_until(from: Weekday, target: Weekday) -> u64 {
    u64::from((7 + target.num_days_from_monday() - from.num_days_from_monday()) % 7)
}

/// The next one, never today: typed on a Thursday, `thursday` is next
/// week's (Q6's placeholder, as Todoist does).
fn bare_weekday(captures: &Captures, ctx: &ParseContext) -> Option<Value> {
    let today = ctx.today();
    let ahead = match days_until(today.weekday(), weekday(captures)?) {
        0 => 7,
        days => days,
    };
    today.checked_add_days(Days::new(ahead)).map(Value::Date)
}

/// The next one, today included.
fn this_weekday(captures: &Captures, ctx: &ParseContext) -> Option<Value> {
    let today = ctx.today();
    let ahead = days_until(today.weekday(), weekday(captures)?);
    today.checked_add_days(Days::new(ahead)).map(Value::Date)
}

/// That day in next week, the British reading S8 graded: on Thursday 24
/// September, `next fri` is 2 October, not the next day. Weeks start on
/// Monday.
fn next_weekday(captures: &Captures, ctx: &ParseContext) -> Option<Value> {
    let monday = next_monday(ctx.today())?;
    monday
        .checked_add_days(Days::new(u64::from(
            weekday(captures)?.num_days_from_monday(),
        )))
        .map(Value::Date)
}

fn next_monday(today: NaiveDate) -> Option<NaiveDate> {
    today.checked_add_days(Days::new(u64::from(
        7 - today.weekday().num_days_from_monday(),
    )))
}

fn first_of_next_month(today: NaiveDate) -> Option<NaiveDate> {
    today.with_day(1)?.checked_add_months(Months::new(1))
}

/// `next week` is its Monday; `end of week` the Friday on or after today;
/// `next month` its 1st (S8 graded it as the same day next month, as a
/// guess it flagged; the brief for this build chose the 1st).
fn period(captures: &Captures, ctx: &ParseContext) -> Option<Value> {
    let today = ctx.today();
    let date = match text(captures, 0) {
        "next week" => next_monday(today)?,
        "next month" => first_of_next_month(today)?,
        "end of week" | "eow" => {
            today.checked_add_days(Days::new(days_until(today.weekday(), Weekday::Fri)))?
        }
        _ => first_of_next_month(today)?.pred_opt()?,
    };
    Some(Value::Date(date))
}

/// Move `from` by `count` of `unit`, back when `back`.
fn shift(from: NaiveDate, count: u32, unit: &str, back: bool) -> Option<NaiveDate> {
    match unit.chars().next()? {
        'd' | 'w' => {
            let days = u64::from(count) * if unit.starts_with('w') { 7 } else { 1 };
            if back {
                from.checked_sub_days(Days::new(days))
            } else {
                from.checked_add_days(Days::new(days))
            }
        }
        _ if back => from.checked_sub_months(Months::new(count)),
        _ => from.checked_add_months(Months::new(count)),
    }
}

fn ahead(captures: &Captures, ctx: &ParseContext) -> Option<Value> {
    let count = number(text(captures, 1))?;
    shift(ctx.today(), count, text(captures, 2), false).map(Value::Date)
}

fn ago(captures: &Captures, ctx: &ParseContext) -> Option<Value> {
    let count = number(text(captures, 1))?;
    shift(ctx.today(), count, text(captures, 2), true).map(Value::Date)
}

fn signed(captures: &Captures, ctx: &ParseContext) -> Option<Value> {
    let back = text(captures, 1) == "-";
    shift(ctx.today(), int(captures, 2)?, text(captures, 3), back).map(Value::Date)
}

fn add_days(date: NaiveDate, days: i64) -> Option<NaiveDate> {
    date.checked_add_signed(chrono::Duration::try_days(days)?)
}

/// A day and month in `year`, or with none given, the next one from
/// today: a day and month already past this year mean next year's.
fn day_of_year(day: u32, month: u32, year: Option<i32>, ctx: &ParseContext) -> Option<Value> {
    let today = ctx.today();
    let date = match year {
        Some(year) => NaiveDate::from_ymd_opt(year, month, day)?,
        None => NaiveDate::from_ymd_opt(today.year(), month, day)
            .filter(|date| *date >= today)
            .or_else(|| NaiveDate::from_ymd_opt(today.year() + 1, month, day))?,
    };
    Some(Value::Date(date))
}

fn day_month(captures: &Captures, ctx: &ParseContext) -> Option<Value> {
    let month = lookup(MONTHS, text(captures, 2))?;
    let year = text(captures, 3).parse().ok();
    day_of_year(int(captures, 1)?, month, year, ctx)
}

fn month_day(captures: &Captures, ctx: &ParseContext) -> Option<Value> {
    let month = lookup(MONTHS, text(captures, 1))?;
    let year = text(captures, 3).parse().ok();
    day_of_year(int(captures, 2)?, month, year, ctx)
}

/// Day first: `12/10` is 12 October (Q8's placeholder, UK order).
fn day_slash_month(captures: &Captures, ctx: &ParseContext) -> Option<Value> {
    day_of_year(int(captures, 1)?, int(captures, 2)?, None, ctx)
}

fn twelve_hour(captures: &Captures, _: &ParseContext) -> Option<Value> {
    let hour = int(captures, 1).filter(|hour| (1..=12).contains(hour))?;
    let minute = if text(captures, 2).is_empty() {
        0
    } else {
        int(captures, 2)?
    };
    let hour = match (hour, text(captures, 3)) {
        (12, "am") => 0,
        (12, _) => 12,
        (hour, "pm") => hour + 12,
        (hour, _) => hour,
    };
    NaiveTime::from_hms_opt(hour, minute, 0).map(Value::Time)
}

fn twenty_four_hour(captures: &Captures, _: &ParseContext) -> Option<Value> {
    NaiveTime::from_hms_opt(int(captures, 1)?, int(captures, 2)?, 0).map(Value::Time)
}

fn named_time(captures: &Captures, _: &ParseContext) -> Option<Value> {
    let hour = if text(captures, 0) == "midnight" {
        0
    } else {
        12
    };
    NaiveTime::from_hms_opt(hour, 0, 0).map(Value::Time)
}
