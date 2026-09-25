//! Dates and times as people type them: `tomorrow`, `fri 17:30`,
//! `in three days`, `12 oct`. Whole-string mode, where the whole input
//! must be a date phrase, reads the date fields and the `--due` and
//! `--reminder` flags; span mode finds the same phrases inside a quick-add
//! title (rung 6a).

mod rules;
pub(crate) mod span;
mod whole_string;
pub(crate) mod words;

use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime};

/// What a phrase is read against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseContext {
    /// Now, in the user's zone: dates and times read as local wall-clock
    /// values there, the way the daemon writes them (D-027).
    pub now: DateTime<FixedOffset>,
    /// Which way a weekday, or a day and month with no year, points.
    pub lean: Lean,
}

/// Which way a phrase that names no week or year points: ahead for a due
/// date or reminder (`fri` is the next Friday), back for "since when"
/// (`fri` is the last one).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Lean {
    #[default]
    Ahead,
    /// The latest such day on or before today: today counts.
    Back,
}

impl ParseContext {
    pub fn new(now: DateTime<FixedOffset>) -> Self {
        Self {
            now,
            lean: Lean::Ahead,
        }
    }

    pub(crate) fn today(&self) -> NaiveDate {
        self.now.date_naive()
    }

    pub(crate) fn local_now(&self) -> NaiveDateTime {
        self.now.naive_local()
    }

    /// A time alone is the next one: today's, or tomorrow's once it has
    /// passed. Exactly now has passed too: a reminder for now would never
    /// ring.
    pub(crate) fn next_at(&self, time: NaiveTime) -> Option<NaiveDateTime> {
        let today = self.today();
        let date = if time > self.local_now().time() {
            today
        } else {
            today.succ_opt()?
        };
        Some(date.and_time(time))
    }
}

/// A resolved date phrase: a day, or a day and a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DueSpec {
    Date(NaiveDate),
    DateTime(NaiveDateTime),
}

impl DueSpec {
    pub fn date(self) -> NaiveDate {
        match self {
            Self::Date(date) => date,
            Self::DateTime(at) => at.date(),
        }
    }

    pub fn time(self) -> Option<NaiveTime> {
        match self {
            Self::Date(_) => None,
            Self::DateTime(at) => Some(at.time()),
        }
    }
}

/// What a field's text says to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reading<T> {
    /// Empty or `-`: remove the value.
    Clear,
    /// `preview` is the value as a person reads it back, such as
    /// `Fri 2 Oct` or `Wed 23 Sep, in the past`.
    Set { value: T, preview: String },
}

/// Why a phrase can't be read, naming what wasn't understood.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct NotUnderstood(pub String);

/// The whole of `input` as a date, or a date and a time.
pub fn read_when(input: &str, ctx: &ParseContext) -> Result<Reading<DueSpec>, NotUnderstood> {
    Ok(match whole_string::read(input, ctx)? {
        None => Reading::Clear,
        Some(spec) => Reading::Set {
            value: spec,
            preview: preview(spec, ctx),
        },
    })
}

/// A due date. It has no time (D-027): a phrase with one is refused.
/// A date in the past is allowed, and its preview says so.
pub fn read_due(input: &str, ctx: &ParseContext) -> Result<Reading<NaiveDate>, NotUnderstood> {
    match read_when(input, ctx)? {
        Reading::Clear => Ok(Reading::Clear),
        Reading::Set {
            value: DueSpec::Date(date),
            preview,
        } => Ok(Reading::Set {
            value: date,
            preview,
        }),
        Reading::Set { .. } => Err(NotUnderstood(
            "a due date has no time; put the time in the reminder".into(),
        )),
    }
}

/// A day in the past, for "since" and "until": `yesterday`, `mon` (the
/// latest Monday, today included), `last week` (its Monday), `12 sep`
/// (the latest 12 September), `3 days ago`, `2026-09-01`. A phrase with a
/// time is refused, and so is nothing at all.
pub fn read_past_date(input: &str, ctx: &ParseContext) -> Result<NaiveDate, NotUnderstood> {
    let back = ParseContext {
        lean: Lean::Back,
        ..*ctx
    };
    match whole_string::read(input, &back)? {
        Some(DueSpec::Date(date)) => Ok(date),
        Some(DueSpec::DateTime(_)) => {
            Err(NotUnderstood("a day is wanted here, without a time".into()))
        }
        None => Err(NotUnderstood(
            "a day is wanted here, such as yesterday, mon or 2026-09-01".into(),
        )),
    }
}

/// The time a reminder given only a day rings at. 09:00 matches
/// Microsoft To Do's own "Tomorrow" reminder.
pub(crate) const DEFAULT_REMINDER_TIME: NaiveTime = match NaiveTime::from_hms_opt(9, 0, 0) {
    Some(time) => time,
    None => panic!("09:00 is a valid time"),
};

/// A reminder: `17:30` alone is today's, or tomorrow's once it has
/// passed; a day alone is that day at [`DEFAULT_REMINDER_TIME`]. A day
/// alone that is today keeps today even when 09:00 has passed, and its
/// preview says so, as a past due date's does, rather than moving to a
/// day nobody typed.
pub fn read_reminder(
    input: &str,
    ctx: &ParseContext,
) -> Result<Reading<NaiveDateTime>, NotUnderstood> {
    Ok(match whole_string::read(input, ctx)? {
        None => Reading::Clear,
        Some(spec) => {
            let at = match spec {
                DueSpec::DateTime(at) => at,
                DueSpec::Date(date) => date.and_time(DEFAULT_REMINDER_TIME),
            };
            Reading::Set {
                value: at,
                preview: preview(DueSpec::DateTime(at), ctx),
            }
        }
    })
}

/// `Fri 2 Oct`, with the year when it isn't this one, the time when
/// there is one, and a note when it's already past.
pub(crate) fn preview(spec: DueSpec, ctx: &ParseContext) -> String {
    let date = spec.date();
    let mut shown = if date.year() == ctx.today().year() {
        date.format("%a %-d %b").to_string()
    } else {
        date.format("%a %-d %b %Y").to_string()
    };
    if let Some(time) = spec.time() {
        shown.push_str(&time.format(" %H:%M").to_string());
    }
    let past = match spec {
        DueSpec::Date(date) => date < ctx.today(),
        DueSpec::DateTime(at) => at < ctx.local_now(),
    };
    if past {
        shown.push_str(", in the past");
    }
    shown
}

#[cfg(test)]
mod tests;
