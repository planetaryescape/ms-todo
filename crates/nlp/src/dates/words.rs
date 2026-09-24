//! The words the date rules know. Exact words, never prefixes: S8 found
//! that matching a word on its first three letters reads `Monitor` as
//! Monday and `Octopus` as October.

use chrono::Weekday;

pub(super) const WEEKDAYS: &[(&str, Weekday)] = &[
    ("monday", Weekday::Mon),
    ("mon", Weekday::Mon),
    ("tuesday", Weekday::Tue),
    ("tues", Weekday::Tue),
    ("tue", Weekday::Tue),
    ("wednesday", Weekday::Wed),
    ("weds", Weekday::Wed),
    ("wed", Weekday::Wed),
    ("thursday", Weekday::Thu),
    ("thurs", Weekday::Thu),
    ("thur", Weekday::Thu),
    ("thu", Weekday::Thu),
    ("friday", Weekday::Fri),
    ("fri", Weekday::Fri),
    ("saturday", Weekday::Sat),
    ("sat", Weekday::Sat),
    ("sunday", Weekday::Sun),
    ("sun", Weekday::Sun),
];

pub(super) const MONTHS: &[(&str, u32)] = &[
    ("january", 1),
    ("jan", 1),
    ("february", 2),
    ("feb", 2),
    ("march", 3),
    ("mar", 3),
    ("april", 4),
    ("apr", 4),
    ("may", 5),
    ("june", 6),
    ("jun", 6),
    ("july", 7),
    ("jul", 7),
    ("august", 8),
    ("aug", 8),
    ("september", 9),
    ("sept", 9),
    ("sep", 9),
    ("october", 10),
    ("oct", 10),
    ("november", 11),
    ("nov", 11),
    ("december", 12),
    ("dec", 12),
];

/// Days from today. `tonight` is today with no time (Q7's placeholder in
/// docs/blueprint/12-open-questions.md). `tom` is tomorrow; Q9's rule
/// that only lowercase `tom` counts, so "Ask Tom" stays a title, is for
/// rung 6's scanner inside titles, not a field that holds a date alone.
pub(super) const RELATIVE_DAYS: &[(&str, i64)] = &[
    ("day after tomorrow", 2),
    ("day before yesterday", -2),
    ("today", 0),
    ("tonight", 0),
    ("tod", 0),
    ("tomorrow", 1),
    ("tmrw", 1),
    ("tom", 1),
    ("yesterday", -1),
];

/// Counts spelled out, one to twenty, and "a" as in "in a week".
pub(super) const NUMBERS: &[(&str, u32)] = &[
    ("a", 1),
    ("an", 1),
    ("one", 1),
    ("two", 2),
    ("three", 3),
    ("four", 4),
    ("five", 5),
    ("six", 6),
    ("seven", 7),
    ("eight", 8),
    ("nine", 9),
    ("ten", 10),
    ("eleven", 11),
    ("twelve", 12),
    ("thirteen", 13),
    ("fourteen", 14),
    ("fifteen", 15),
    ("sixteen", 16),
    ("seventeen", 17),
    ("eighteen", 18),
    ("nineteen", 19),
    ("twenty", 20),
];

/// The words of `table` as a regex alternation, longest first, so the
/// leftmost-first match takes `tomorrow` over `tom`.
pub(super) fn alternation<T>(table: &[(&str, T)]) -> String {
    let mut words: Vec<&str> = table.iter().map(|(word, _)| *word).collect();
    words.sort_by_key(|word| std::cmp::Reverse(word.len()));
    words.join("|")
}

pub(super) fn lookup<T: Copy>(table: &[(&str, T)], word: &str) -> Option<T> {
    table
        .iter()
        .find(|(known, _)| *known == word)
        .map(|(_, value)| *value)
}

/// A count as digits or spelled out.
pub(super) fn number(text: &str) -> Option<u32> {
    text.parse().ok().or_else(|| lookup(NUMBERS, text))
}
