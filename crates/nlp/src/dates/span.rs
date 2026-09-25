//! Span mode: a date phrase starting at a given word of a title, read by
//! the same rule table as whole-string mode (D-026). The caller, quick
//! add's scanner, picks the words to try, masks what earlier passes
//! claimed and applies the guards that need the original text (`Tom`,
//! `Sat nav`); this module only knows where a phrase ends and what it
//! means.
//!
//! `scan` is the title lowercased (ASCII only, so byte offsets are the
//! original's) with claimed bytes replaced by [`MASK`].

use chrono::{NaiveDate, NaiveDateTime, NaiveTime};

use super::ParseContext;
use super::rules::{DATE_RULES, Rule, TIME_RULES, Value, longest};

/// What a claimed byte reads as: never part of a word or a phrase, and
/// the start of a new word after it.
pub(crate) const MASK: char = '\u{1}';

/// What a byte kept literally (quoted or escaped) reads as: never part of
/// a phrase, and not a word boundary either, so `\!9am` stays text.
pub(crate) const LITERAL: char = '\u{2}';

/// A phrase's meaning: a day, a time of day, or both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum When {
    Date(NaiveDate),
    Time(NaiveTime),
    DateTime(NaiveDateTime),
}

impl When {
    pub fn date(self) -> Option<NaiveDate> {
        match self {
            Self::Date(date) => Some(date),
            Self::DateTime(at) => Some(at.date()),
            Self::Time(_) => None,
        }
    }

    pub fn time(self) -> Option<NaiveTime> {
        match self {
            Self::Time(time) => Some(time),
            Self::DateTime(at) => Some(at.time()),
            Self::Date(_) => None,
        }
    }
}

/// The phrase that starts at byte `at` of `scan`, and the byte it ends
/// at: a date, optionally followed by a time (`fri 5pm`, `fri at 5pm`);
/// or a time, optionally followed by a date (`9am tomorrow`). `on` may
/// lead a date and `at` a time, and is part of the phrase. A phrase must
/// end at the end of a word: `12/10/26` and `Friday's` aren't read.
pub(crate) fn phrase_at(scan: &str, at: usize, ctx: &ParseContext) -> Option<(When, usize)> {
    let rest = scan.get(at..)?;
    if rest.starts_with("on ")
        && let Some(found) = date_first(scan, at + 3, ctx)
    {
        return Some(found);
    }
    if rest.starts_with("at ")
        && let Some(found) = time_first(scan, at + 3, ctx)
    {
        return Some(found);
    }
    date_first(scan, at, ctx).or_else(|| time_first(scan, at, ctx))
}

/// A date phrase alone at `at`, with no time after it: `until 31 dec`.
pub(crate) fn date_at(scan: &str, at: usize, ctx: &ParseContext) -> Option<(NaiveDate, usize)> {
    match matched(&DATE_RULES, scan, at, ctx)? {
        (Value::Date(date), end) if ends_word(scan, end) => Some((date, end)),
        _ => None,
    }
}

/// A time of day alone at `at`, with an optional leading `at`.
pub(crate) fn time_at(scan: &str, at: usize, ctx: &ParseContext) -> Option<(NaiveTime, usize)> {
    let at = if scan.get(at..)?.starts_with("at ") {
        at + 3
    } else {
        at
    };
    match matched(&TIME_RULES, scan, at, ctx)? {
        (Value::Time(time), end) if ends_word(scan, end) => Some((time, end)),
        _ => None,
    }
}

fn date_first(scan: &str, at: usize, ctx: &ParseContext) -> Option<(When, usize)> {
    let (value, end) = matched(&DATE_RULES, scan, at, ctx)?;
    let date = match value {
        Value::Date(date) => date,
        Value::DateTime(at) => return ends_word(scan, end).then_some((When::DateTime(at), end)),
        Value::Time(_) => return None,
    };
    for joiner in [" at ", " "] {
        if scan[end..].starts_with(joiner)
            && let Some((Value::Time(time), timed)) =
                matched(&TIME_RULES, scan, end + joiner.len(), ctx)
            && ends_word(scan, timed)
        {
            return Some((When::DateTime(date.and_time(time)), timed));
        }
    }
    ends_word(scan, end).then_some((When::Date(date), end))
}

fn time_first(scan: &str, at: usize, ctx: &ParseContext) -> Option<(When, usize)> {
    let (Value::Time(time), end) = matched(&TIME_RULES, scan, at, ctx)? else {
        return None;
    };
    for joiner in [" on ", " "] {
        if scan[end..].starts_with(joiner)
            && let Some((Value::Date(date), dated)) =
                matched(&DATE_RULES, scan, end + joiner.len(), ctx)
            && ends_word(scan, dated)
        {
            return Some((When::DateTime(date.and_time(time)), dated));
        }
    }
    ends_word(scan, end).then_some((When::Time(time), end))
}

/// The longest rule matching at `at`, and the byte it ends at. A shape
/// that isn't a real date (`31/02`) is no match: in a title it's text.
fn matched(rules: &[Rule], scan: &str, at: usize, ctx: &ParseContext) -> Option<(Value, usize)> {
    let (value, length) = longest(rules, scan.get(at..)?, ctx).ok()??;
    Some((value, at + length))
}

/// Whether a phrase ending at byte `end` ends a word: the end of the
/// text, a space, a claimed byte, or punctuation that itself ends the
/// word (`fri, then`). An apostrophe, a slash or a dot inside a word
/// (`Friday's`, `12/10/26`, `12.10.2026`) doesn't.
pub(crate) fn ends_word(scan: &str, end: usize) -> bool {
    let mut after = scan[end..].chars();
    match after.next() {
        None => true,
        Some(next) if next.is_whitespace() || next == MASK => true,
        Some(',' | '.' | ';' | ':' | '!' | '?' | ')') => after
            .next()
            .is_none_or(|next| next.is_whitespace() || next == MASK),
        Some(_) => false,
    }
}
