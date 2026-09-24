//! Whole-string mode: the input must be a date phrase from end to end,
//! as in a date field. A date, optionally followed by `at` and a time; or
//! a time, optionally followed by a date. `on` may lead a date and `at` a
//! time.

use chrono::Days;

use super::rules::{DATE_RULES, Rule, TIME_RULES, Value};
use super::{DueSpec, NotUnderstood, ParseContext};

/// `None` for empty or `-`, which clear the field.
pub(super) fn read(input: &str, ctx: &ParseContext) -> Result<Option<DueSpec>, NotUnderstood> {
    let text = input
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    if text.is_empty() || text == "-" {
        return Ok(None);
    }
    let dated = strip_word(&text, "on");
    if let Some((value, rest)) = longest(&DATE_RULES, dated, ctx)? {
        if rest.is_empty() {
            return Ok(Some(match value {
                Value::DateTime(at) => DueSpec::DateTime(at),
                Value::Date(date) => DueSpec::Date(date),
                Value::Time(_) => return Err(not_understood(&text)),
            }));
        }
        let rest = strip_word(rest, "at");
        return match (value, whole(&TIME_RULES, rest, ctx)?) {
            (Value::Date(date), Some(Value::Time(time))) => {
                Ok(Some(DueSpec::DateTime(date.and_time(time))))
            }
            _ => Err(not_understood(rest)),
        };
    }
    let timed = strip_word(&text, "at");
    if let Some((Value::Time(time), rest)) = longest(&TIME_RULES, timed, ctx)? {
        if rest.is_empty() {
            // A time alone is the next one: today's, or tomorrow's once
            // it has passed.
            let today = ctx.today();
            let date = if time > ctx.local_now().time() {
                today
            } else {
                today
                    .checked_add_days(Days::new(1))
                    .ok_or_else(|| not_understood(&text))?
            };
            return Ok(Some(DueSpec::DateTime(date.and_time(time))));
        }
        let rest = strip_word(rest, "on");
        return match whole(&DATE_RULES, rest, ctx)? {
            Some(Value::Date(date)) => Ok(Some(DueSpec::DateTime(date.and_time(time)))),
            _ => Err(not_understood(rest)),
        };
    }
    Err(not_understood(&text))
}

/// The longest match of any rule at the start of `text`, and what's left
/// after it, trimmed. A match that isn't a real date or time, such as
/// `31/02`, is an error when nothing else matches.
fn longest<'t>(
    rules: &[Rule],
    text: &'t str,
    ctx: &ParseContext,
) -> Result<Option<(Value, &'t str)>, NotUnderstood> {
    let mut best: Option<(usize, Value)> = None;
    let mut unreal = None;
    for rule in rules {
        let Some(captures) = rule.regex.captures(text) else {
            continue;
        };
        let end = captures.get(0).map_or(0, |found| found.end());
        match (rule.resolve)(&captures, ctx) {
            Some(value) if best.is_none_or(|(longest, _)| end > longest) => {
                best = Some((end, value));
            }
            Some(_) => {}
            None => unreal = Some(&text[..end]),
        }
    }
    match (best, unreal) {
        (Some((end, value)), _) => Ok(Some((value, text[end..].trim_start()))),
        (None, Some(unreal)) => Err(NotUnderstood(format!(
            "\"{unreal}\" isn't a real date or time"
        ))),
        (None, None) => Ok(None),
    }
}

/// A rule that matches all of `text`.
fn whole(rules: &[Rule], text: &str, ctx: &ParseContext) -> Result<Option<Value>, NotUnderstood> {
    Ok(longest(rules, text, ctx)?
        .filter(|(_, rest)| rest.is_empty())
        .map(|(value, _)| value))
}

/// `text` without a leading `word` and the space after it.
fn strip_word<'t>(text: &'t str, word: &str) -> &'t str {
    text.strip_prefix(word)
        .and_then(|rest| rest.strip_prefix(' '))
        .unwrap_or(text)
}

fn not_understood(what: &str) -> NotUnderstood {
    NotUnderstood(format!("didn't understand \"{what}\""))
}
