//! Dates and times as people type them: `tomorrow`, `fri 17:30`,
//! `in three days`, `12 oct`. Rung 6's quick add will find these inside a
//! title (span mode); this build has whole-string mode only, where the
//! whole input must be a date phrase, for the date fields and the
//! `--due` and `--reminder` flags.

mod rules;
mod whole_string;
mod words;

use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime};

/// What a phrase is read against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseContext {
    /// Now, in the user's zone: dates and times read as local wall-clock
    /// values there, the way the daemon writes them (D-027).
    pub now: DateTime<FixedOffset>,
}

impl ParseContext {
    pub fn new(now: DateTime<FixedOffset>) -> Self {
        Self { now }
    }

    fn today(&self) -> NaiveDate {
        self.now.date_naive()
    }

    fn local_now(&self) -> NaiveDateTime {
        self.now.naive_local()
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

/// A reminder, which needs a time: `17:30` alone is today's, or
/// tomorrow's once it has passed; a day alone is refused rather than
/// given a time nobody typed.
pub fn read_reminder(
    input: &str,
    ctx: &ParseContext,
) -> Result<Reading<NaiveDateTime>, NotUnderstood> {
    match read_when(input, ctx)? {
        Reading::Clear => Ok(Reading::Clear),
        Reading::Set {
            value: DueSpec::DateTime(at),
            preview,
        } => Ok(Reading::Set { value: at, preview }),
        Reading::Set { .. } => Err(NotUnderstood(
            "a reminder needs a time too, as in \"tomorrow 9am\"".into(),
        )),
    }
}

/// `Fri 2 Oct`, with the year when it isn't this one, the time when
/// there is one, and a note when it's already past.
fn preview(spec: DueSpec, ctx: &ParseContext) -> String {
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
