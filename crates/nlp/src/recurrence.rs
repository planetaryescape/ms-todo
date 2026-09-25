//! `every …`: a small grammar that maps straight onto Graph's
//! `patternedRecurrence` (docs/blueprint/06-natural-language.md#recurrence-custom-parser-and-why).
//! No date crate reads recurrences, and none targets Graph's model, so
//! this is custom: one regex per shape, like the date rules, and a
//! resolver per shape.
//!
//! What's read has no zone: the daemon adds `range.recurrenceTimeZone`,
//! the zone it writes the due date in, to every recurrence it sends
//! (S12: without it, Graph moved the due date a day on).

use std::sync::LazyLock;

use chrono::{Datelike, Months, NaiveDate, NaiveTime, Weekday};
use regex::{Captures, Regex};
use serde_json::{Value, json};

use crate::dates::ParseContext;
use crate::dates::span::{date_at, ends_word, time_at};
use crate::dates::words::{MONTHS, NUMBERS, WEEKDAYS, alternation, lookup, number};

/// A recurrence as read, anchored to its first due date, `start`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recurrence {
    pub pattern: Pattern,
    pub end: RecurrenceEnd,
    /// `range.startDate`, and the task's first due date.
    pub start: NaiveDate,
}

/// Graph's pattern types. A day left out (`every week`, `every month`,
/// `every year`) is the start date's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pattern {
    Daily {
        interval: u32,
    },
    /// Days in Monday-first order, no repeats; empty is the start's
    /// weekday.
    Weekly {
        interval: u32,
        days: Vec<Weekday>,
    },
    AbsoluteMonthly {
        interval: u32,
        day: Option<u32>,
    },
    RelativeMonthly {
        interval: u32,
        index: WeekIndex,
        day: Weekday,
    },
    /// `(month, day)`.
    AbsoluteYearly {
        interval: u32,
        date: Option<(u32, u32)>,
    },
}

/// Which of a month's weekdays: Graph's `weekIndex`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeekIndex {
    First,
    Second,
    Third,
    Fourth,
    Last,
}

/// Graph's range types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecurrenceEnd {
    Never,
    /// The last day an occurrence may fall on.
    Until(NaiveDate),
    /// This many occurrences.
    Count(u32),
}

/// What `every …` at a word reads as: the pattern and end, the time of
/// day if one followed, and the byte the phrase ends at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Read {
    pub pattern: Pattern,
    pub end: RecurrenceEnd,
    pub time: Option<NaiveTime>,
    pub until: usize,
}

impl Recurrence {
    /// The first day on or after `from` that the pattern falls on: a new
    /// recurring task's first due date when no date was typed.
    pub fn first_on_or_after(pattern: &Pattern, from: NaiveDate) -> NaiveDate {
        let found = match pattern {
            Pattern::Daily { .. } => Some(from),
            Pattern::Weekly { days, .. } if days.is_empty() => Some(from),
            Pattern::Weekly { days, .. } => from
                .iter_days()
                .take(7)
                .find(|date| days.contains(&date.weekday())),
            Pattern::AbsoluteMonthly { day: None, .. }
            | Pattern::AbsoluteYearly { date: None, .. } => Some(from),
            Pattern::AbsoluteMonthly { day: Some(day), .. } => months_from(from)
                .filter_map(|first| first.with_day(*day))
                .find(|date| *date >= from),
            Pattern::RelativeMonthly { index, day, .. } => months_from(from)
                .filter_map(|first| nth_weekday(first, *index, *day))
                .find(|date| *date >= from),
            Pattern::AbsoluteYearly {
                date: Some((month, day)),
                ..
            } => (0..8)
                .filter_map(|years| NaiveDate::from_ymd_opt(from.year() + years, *month, *day))
                .find(|date| *date >= from),
        };
        found.unwrap_or(from)
    }

    /// Graph's `patternedRecurrence`, less `range.recurrenceTimeZone`,
    /// which the daemon adds.
    pub fn to_graph(&self) -> Value {
        let (kind, interval) = match &self.pattern {
            Pattern::Daily { interval } => ("daily", interval),
            Pattern::Weekly { interval, .. } => ("weekly", interval),
            Pattern::AbsoluteMonthly { interval, .. } => ("absoluteMonthly", interval),
            Pattern::RelativeMonthly { interval, .. } => ("relativeMonthly", interval),
            Pattern::AbsoluteYearly { interval, .. } => ("absoluteYearly", interval),
        };
        let mut pattern = json!({ "type": kind, "interval": interval });
        match &self.pattern {
            Pattern::Daily { .. } => {}
            Pattern::Weekly { .. } => {
                pattern["daysOfWeek"] = json!(
                    self.weekdays()
                        .iter()
                        .map(|day| weekday_name(*day))
                        .collect::<Vec<_>>()
                );
                // Q8's UK placeholder: weeks start on Monday.
                pattern["firstDayOfWeek"] = json!("monday");
            }
            Pattern::AbsoluteMonthly { day, .. } => {
                pattern["dayOfMonth"] = json!(day.unwrap_or(self.start.day()));
            }
            Pattern::RelativeMonthly { index, day, .. } => {
                pattern["daysOfWeek"] = json!([weekday_name(*day)]);
                pattern["index"] = json!(index.name());
            }
            Pattern::AbsoluteYearly { date, .. } => {
                let (month, day) = date.unwrap_or((self.start.month(), self.start.day()));
                pattern["month"] = json!(month);
                pattern["dayOfMonth"] = json!(day);
            }
        }
        let start = self.start.format("%Y-%m-%d").to_string();
        let range = match self.end {
            RecurrenceEnd::Never => json!({ "type": "noEnd", "startDate": start }),
            RecurrenceEnd::Until(until) => json!({
                "type": "endDate",
                "startDate": start,
                "endDate": until.format("%Y-%m-%d").to_string(),
            }),
            RecurrenceEnd::Count(count) => json!({
                "type": "numbered",
                "startDate": start,
                "numberOfOccurrences": count,
            }),
        };
        json!({ "pattern": pattern, "range": range })
    }

    /// As a person reads it back: `every month on the 1st`, `every 2 weeks
    /// on Mon, Wed`, `every last Fri until 31 Dec`.
    pub fn describe(&self) -> String {
        let every = |interval: u32, unit: &str| match interval {
            1 => format!("every {unit}"),
            2 => format!("every other {unit}"),
            n => format!("every {n} {unit}s"),
        };
        let mut text = match &self.pattern {
            Pattern::Daily { interval } => every(*interval, "day"),
            Pattern::Weekly { interval, .. } => {
                let days = self.weekdays();
                match (interval, days.as_slice()) {
                    (1, days) if days == WORKING_WEEK => "every weekday".to_owned(),
                    (1, [Weekday::Sat, Weekday::Sun]) => "every weekend".to_owned(),
                    (1, days) => format!("every {}", day_list(days)),
                    (interval, days) => {
                        format!("{} on {}", every(*interval, "week"), day_list(days))
                    }
                }
            }
            Pattern::AbsoluteMonthly { interval, day } => format!(
                "{} on the {}",
                every(*interval, "month"),
                ordinal(day.unwrap_or(self.start.day()))
            ),
            Pattern::RelativeMonthly {
                interval: 1,
                index,
                day,
            } => format!("every {} {}", index.name(), day_list(&[*day])),
            Pattern::RelativeMonthly {
                interval,
                index,
                day,
            } => format!(
                "{} on the {} {}",
                every(*interval, "month"),
                index.name(),
                day_list(&[*day])
            ),
            Pattern::AbsoluteYearly { interval, date } => {
                let (month, day) = date.unwrap_or((self.start.month(), self.start.day()));
                let shown = NaiveDate::from_ymd_opt(2000, month, day)
                    .map(|date| date.format("%-d %b").to_string())
                    .unwrap_or_default();
                format!("{} on {shown}", every(*interval, "year"))
            }
        };
        match self.end {
            RecurrenceEnd::Never => {}
            RecurrenceEnd::Until(until) => {
                text.push_str(&until.format(" until %-d %b %Y").to_string());
            }
            RecurrenceEnd::Count(1) => text.push_str(", once"),
            RecurrenceEnd::Count(count) => text.push_str(&format!(", {count} times")),
        }
        text
    }

    /// A weekly pattern's days, the start's when none were typed.
    fn weekdays(&self) -> Vec<Weekday> {
        match &self.pattern {
            Pattern::Weekly { days, .. } if days.is_empty() => vec![self.start.weekday()],
            Pattern::Weekly { days, .. } => days.clone(),
            _ => Vec::new(),
        }
    }
}

impl WeekIndex {
    fn name(self) -> &'static str {
        match self {
            Self::First => "first",
            Self::Second => "second",
            Self::Third => "third",
            Self::Fourth => "fourth",
            Self::Last => "last",
        }
    }
}

/// The 1st of `from`'s month and of the next twelve.
fn months_from(from: NaiveDate) -> impl Iterator<Item = NaiveDate> {
    let first = from.with_day(1);
    (0..13).filter_map(move |months| first?.checked_add_months(Months::new(months)))
}

/// The `index` `day` of the month that starts on `first`.
fn nth_weekday(first: NaiveDate, index: WeekIndex, day: Weekday) -> Option<NaiveDate> {
    let n = match index {
        WeekIndex::First => 1,
        WeekIndex::Second => 2,
        WeekIndex::Third => 3,
        WeekIndex::Fourth => 4,
        WeekIndex::Last => {
            let last = first.checked_add_months(Months::new(1))?.pred_opt()?;
            let back = last.weekday().days_since(day);
            return last.checked_sub_days(chrono::Days::new(u64::from(back)));
        }
    };
    NaiveDate::from_weekday_of_month_opt(first.year(), first.month(), day, n)
}

fn weekday_name(day: Weekday) -> &'static str {
    match day {
        Weekday::Mon => "monday",
        Weekday::Tue => "tuesday",
        Weekday::Wed => "wednesday",
        Weekday::Thu => "thursday",
        Weekday::Fri => "friday",
        Weekday::Sat => "saturday",
        Weekday::Sun => "sunday",
    }
}

fn day_list(days: &[Weekday]) -> String {
    days.iter()
        .map(|day| day.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn ordinal(day: u32) -> String {
    let suffix = match (day % 10, day % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{day}{suffix}")
}

/// `every weekday`.
const WORKING_WEEK: [Weekday; 5] = [
    Weekday::Mon,
    Weekday::Tue,
    Weekday::Wed,
    Weekday::Thu,
    Weekday::Fri,
];

struct Shape {
    regex: Regex,
    resolve: fn(&Captures) -> Option<Pattern>,
}

/// The shapes after `every `, longest match wins.
static SHAPES: LazyLock<Vec<Shape>> = LazyLock::new(|| {
    let weekday = alternation(WEEKDAYS);
    let month = alternation(MONTHS);
    // Spelled counts, but not "a" or "an": "every a week" isn't English.
    let spelled: Vec<(&str, u32)> = NUMBERS
        .iter()
        .copied()
        .filter(|(word, _)| word.len() > 2)
        .collect();
    let count = format!(r"\d+|other|{}", alternation(&spelled));
    let days = format!(r"(?:{weekday})(?:(?:, ?|,? and | ?& ?| )(?:{weekday}))*");
    let index = "first|second|third|fourth|last|1st|2nd|3rd|4th";
    let of_month = "(?: (?:day )?of (?:the|each|every) month)?";
    vec![
        shape(&format!(r"(?:({count}) )?days?"), |c| {
            Some(Pattern::Daily {
                interval: interval(c, 1)?,
            })
        }),
        shape("weekdays?", |_| {
            Some(Pattern::Weekly {
                interval: 1,
                days: WORKING_WEEK.to_vec(),
            })
        }),
        shape("weekends?", |_| {
            Some(Pattern::Weekly {
                interval: 1,
                days: vec![Weekday::Sat, Weekday::Sun],
            })
        }),
        shape(&format!("(?:({count}) )?weeks?(?: on ({days}))?"), |c| {
            Some(Pattern::Weekly {
                interval: interval(c, 1)?,
                days: weekdays_in(text(c, 2)),
            })
        }),
        shape(&format!("({days})"), |c| {
            Some(Pattern::Weekly {
                interval: 1,
                days: weekdays_in(text(c, 1)),
            })
        }),
        shape(
            &format!(
                r"(?:({count}) )?months?(?: on the (\d{{1,2}})(?:st|nd|rd|th)| on the ({index}) ({weekday}))?"
            ),
            |c| {
                let interval = interval(c, 1)?;
                if !text(c, 3).is_empty() {
                    return Some(Pattern::RelativeMonthly {
                        interval,
                        index: week_index(text(c, 3))?,
                        day: lookup(WEEKDAYS, text(c, 4))?,
                    });
                }
                Some(Pattern::AbsoluteMonthly {
                    interval,
                    day: day_of_month(text(c, 2))?,
                })
            },
        ),
        shape(&format!(r"(\d{{1,2}})(?:st|nd|rd|th){of_month}"), |c| {
            Some(Pattern::AbsoluteMonthly {
                interval: 1,
                day: Some(day_of_month(text(c, 1))??),
            })
        }),
        shape(&format!("({index}) ({weekday}){of_month}"), |c| {
            Some(Pattern::RelativeMonthly {
                interval: 1,
                index: week_index(text(c, 1))?,
                day: lookup(WEEKDAYS, text(c, 2))?,
            })
        }),
        shape(&format!(r"(?:({count}) )?years?"), |c| {
            Some(Pattern::AbsoluteYearly {
                interval: interval(c, 1)?,
                date: None,
            })
        }),
        shape(
            &format!(
                r"(\d{{1,2}})(?:st|nd|rd|th)? ({month})|({month}) (\d{{1,2}})(?:st|nd|rd|th)?"
            ),
            |c| {
                let (day, month) = if text(c, 1).is_empty() {
                    (text(c, 4), text(c, 3))
                } else {
                    (text(c, 1), text(c, 2))
                };
                let month = lookup(MONTHS, month)?;
                let day: u32 = day.parse().ok()?;
                // Checked against a leap year, so 29 Feb is allowed.
                NaiveDate::from_ymd_opt(2000, month, day)?;
                Some(Pattern::AbsoluteYearly {
                    interval: 1,
                    date: Some((month, day)),
                })
            },
        ),
    ]
});

static COUNT_END: LazyLock<Regex> = LazyLock::new(|| {
    // The patterns are constants, so a bad one fails every test.
    Regex::new(r"^(?: for)? (\d+) (?:times|time|occurrences)").expect("a valid count")
});

fn shape(pattern: &str, resolve: fn(&Captures) -> Option<Pattern>) -> Shape {
    let regex = Regex::new(&format!("^(?:{pattern})")).expect("a valid recurrence shape");
    Shape { regex, resolve }
}

fn text<'h>(captures: &Captures<'h>, group: usize) -> &'h str {
    captures.get(group).map_or("", |found| found.as_str())
}

/// The count in `group`, 1 when there's none; zero is no match.
fn interval(captures: &Captures, group: usize) -> Option<u32> {
    let count = match text(captures, group) {
        "" => 1,
        "other" => 2,
        typed => number(typed).filter(|count| *count > 0)?,
    };
    Some(count)
}

/// A day of the month, 1–31; `Some(None)` when none was typed.
fn day_of_month(typed: &str) -> Option<Option<u32>> {
    if typed.is_empty() {
        return Some(None);
    }
    let day: u32 = typed.parse().ok()?;
    (1..=31).contains(&day).then_some(Some(day))
}

fn week_index(typed: &str) -> Option<WeekIndex> {
    Some(match typed {
        "first" | "1st" => WeekIndex::First,
        "second" | "2nd" => WeekIndex::Second,
        "third" | "3rd" => WeekIndex::Third,
        "fourth" | "4th" => WeekIndex::Fourth,
        "last" => WeekIndex::Last,
        _ => return None,
    })
}

/// Every weekday named in `typed`, Monday first, once each.
fn weekdays_in(typed: &str) -> Vec<Weekday> {
    let mut days: Vec<Weekday> = typed
        .split(|ch: char| !ch.is_ascii_alphabetic())
        .filter_map(|word| lookup(WEEKDAYS, word))
        .collect();
    days.sort_by_key(Weekday::num_days_from_monday);
    days.dedup();
    days
}

/// `every …` or `daily` at byte `at` of `scan` (see `dates::span`),
/// with what may follow it: `until <date>`, `for N times`, and a time
/// (`at 9am`), in any order.
pub(crate) fn read_at(scan: &str, at: usize, ctx: &ParseContext) -> Option<Read> {
    let rest = scan.get(at..)?;
    let (pattern, mut until) = if rest.starts_with("daily") && ends_word(scan, at + 5) {
        (Pattern::Daily { interval: 1 }, at + 5)
    } else {
        let after = rest.starts_with("every ").then_some(at + 6)?;
        let mut best: Option<(usize, Pattern)> = None;
        for shape in SHAPES.iter() {
            let Some(captures) = shape.regex.captures(&scan[after..]) else {
                continue;
            };
            let end = after + captures.get(0).map_or(0, |found| found.end());
            if !ends_word(scan, end) || best.as_ref().is_some_and(|(longest, _)| end <= *longest) {
                continue;
            }
            if let Some(pattern) = (shape.resolve)(&captures) {
                best = Some((end, pattern));
            }
        }
        let (end, pattern) = best?;
        (pattern, end)
    };
    let mut end = RecurrenceEnd::Never;
    let mut time = None;
    for _ in 0..3 {
        let rest = &scan[until..];
        if end == RecurrenceEnd::Never {
            if rest.starts_with(" until ")
                && let Some((date, after)) = date_at(scan, until + 7, ctx)
            {
                end = RecurrenceEnd::Until(date);
                until = after;
                continue;
            }
            if let Some(captures) = COUNT_END.captures(rest) {
                let after = until + captures.get(0).map_or(0, |found| found.end());
                if let Some(count) = text(&captures, 1).parse().ok().filter(|n| *n > 0)
                    && ends_word(scan, after)
                {
                    end = RecurrenceEnd::Count(count);
                    until = after;
                    continue;
                }
            }
        }
        if time.is_none()
            && rest.starts_with(' ')
            && let Some((at, after)) = time_at(scan, until + 1, ctx)
        {
            time = Some(at);
            until = after;
            continue;
        }
        break;
    }
    Some(Read {
        pattern,
        end,
        time,
        until,
    })
}
