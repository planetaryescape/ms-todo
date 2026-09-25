//! The deterministic reader: ordered passes over the input, each masking
//! the bytes it claims so later passes can't read them again (06). Quotes
//! and escapes first, then `#List`, `@label`, `p1`–`p4`, `+myday`,
//! `every …`, `!reminder`, `start <date>`, and last the bare date and time
//! phrases. That order is what stops `every mon` or `!9am` from also
//! being the due date.

use std::ops::Range;

use chrono::{NaiveDate, NaiveDateTime, NaiveTime};

use super::{ListRef, ParsedTask, QuickAddContext, QuickAddParser, Span, SpanKind};
use crate::dates::span::{LITERAL, MASK, When, ends_word, phrase_at};
use crate::dates::{DEFAULT_REMINDER_TIME, DueSpec, ParseContext, preview};
use crate::importance::read_importance;
use crate::recurrence::{self, Recurrence, RecurrenceEnd};

#[derive(Clone, Copy, Debug, Default)]
pub struct DeterministicParser;

impl QuickAddParser for DeterministicParser {
    fn parse(&self, input: &str, ctx: &QuickAddContext) -> ParsedTask {
        let mut scan = Scan::new(input);
        let mut task = ParsedTask::default();
        scan.quotes_and_escapes();
        scan.list(ctx, &mut task);
        scan.labels(ctx, &mut task);
        scan.priority(&mut task);
        scan.my_day(&mut task);
        let repeats = scan.recurrence(ctx);
        let reminder = scan.reminder(ctx, &mut task);
        let start = scan.start(ctx);
        let dates = scan.dates(ctx, &mut task);
        settle(&mut task, ctx, repeats, reminder, start, dates);
        scan.spans.sort_by_key(|span| span.start);
        task.title = scan.title();
        task.spans = scan.spans;
        task
    }
}

/// Words that are a date only in lower case, so `Ask Tom` and `Sat nav`
/// stay titles (Q9's placeholder).
const LOWERCASE_ONLY: &[&str] = &["tom", "tod", "sat"];

struct Scan<'i> {
    input: &'i str,
    /// `input` lowercased (ASCII only, so offsets match), with each
    /// claimed byte replaced by `MASK`.
    scan: String,
    spans: Vec<Span>,
}

impl<'i> Scan<'i> {
    fn new(input: &'i str) -> Self {
        Self {
            input,
            scan: input.to_ascii_lowercase(),
            spans: Vec::new(),
        }
    }

    fn mask(&mut self, range: Range<usize>, with: char) {
        let masked = with.to_string().repeat(range.len());
        self.scan.replace_range(range, &masked);
    }

    /// A recognised part: taken out of the title and masked.
    fn claim(&mut self, range: Range<usize>, kind: SpanKind) {
        self.spans.push(Span {
            start: range.start,
            end: range.end,
            kind,
        });
        self.mask(range, MASK);
    }

    /// Where words start: after the start, a space or a claimed byte,
    /// at a byte no pass has claimed.
    fn word_starts(&self) -> Vec<usize> {
        let mut starts = Vec::new();
        let mut before = None;
        for (at, ch) in self.scan.char_indices() {
            let starts_word = before.is_none_or(|prev: char| prev.is_whitespace() || prev == MASK);
            if starts_word && !ch.is_whitespace() && ch != MASK && ch != LITERAL {
                starts.push(at);
            }
            before = Some(ch);
        }
        starts
    }

    /// `"…"` is literal: its marks go and what's inside stays in the title
    /// untouched. `\#`, `\@`, `\!`, `\*`, `\+`, `\"` and `\\` are the
    /// character itself. A quote right after `#` or `@` is a name's, for
    /// those passes; an unclosed one is an ordinary character.
    fn quotes_and_escapes(&mut self) {
        let bytes = self.input.as_bytes();
        let mut at = 0;
        while at < bytes.len() {
            match bytes[at] {
                b'\\'
                    if matches!(
                        bytes.get(at + 1),
                        Some(b'#' | b'@' | b'!' | b'*' | b'+' | b'"' | b'\\')
                    ) =>
                {
                    self.claim(at..at + 1, SpanKind::Syntax);
                    self.mask(at + 1..at + 2, LITERAL);
                    at += 2;
                }
                b'"' if at == 0 || !matches!(bytes[at - 1], b'#' | b'@') => {
                    let Some(close) = self.input[at + 1..].find('"').map(|to| at + 1 + to) else {
                        break;
                    };
                    self.claim(at..at + 1, SpanKind::Syntax);
                    self.mask(at + 1..close, LITERAL);
                    self.claim(close..close + 1, SpanKind::Syntax);
                    at = close + 1;
                }
                b'"' => {
                    // A name's quote: skip past its close, if any.
                    at = self.input[at + 1..]
                        .find('"')
                        .map_or(bytes.len(), |to| at + to + 2);
                }
                _ => at += 1,
            }
        }
    }

    /// `#Name` or `#"Two words"` at `at`: the name and where the token
    /// ends. Trailing punctuation isn't part of a bare name.
    fn sigil_name(&self, at: usize) -> Option<(String, usize)> {
        let rest = &self.scan[at + 1..];
        if let Some(quoted) = rest.strip_prefix('"') {
            let close = quoted.find('"')?;
            let end = at + 2 + close + 1;
            let name = self.input[at + 2..end - 1].trim().to_owned();
            return (!name.is_empty() && ends_word(&self.scan, end)).then_some((name, end));
        }
        let length = rest
            .find(|ch: char| ch.is_whitespace() || ch == MASK || ch == LITERAL)
            .unwrap_or(rest.len());
        let word = rest[..length].trim_end_matches([',', '.', ';', ':', '!', '?', ')']);
        let end = at + 1 + word.len();
        (!word.is_empty()).then(|| (self.input[at + 1..end].to_owned(), end))
    }

    /// `#List`: a case-insensitive name, or a prefix only one list has.
    /// The first that names a list wins; anything else stays in the title
    /// with a warning.
    fn list(&mut self, ctx: &QuickAddContext, task: &mut ParsedTask) {
        for at in self.word_starts() {
            if !self.scan[at..].starts_with('#') {
                continue;
            }
            let Some((name, end)) = self.sigil_name(at) else {
                continue;
            };
            let typed = &self.input[at..end];
            match find_list(ctx.lists, &name) {
                Ok(list) if task.list.is_none() => {
                    task.list = Some(list.clone());
                    self.claim(at..end, SpanKind::List);
                }
                Ok(_) => task.warnings.push(format!(
                    "{typed}: only one list can be named, so it stays in the title"
                )),
                Err(why) => task
                    .warnings
                    .push(format!("{typed}: {why}, so it stays in the title")),
            }
        }
    }

    /// `@label`: a category, with its known spelling when it's one of the
    /// user's. An unknown one is kept and warned about: Graph stores any
    /// name, but Outlook shows it without a colour until it's created.
    fn labels(&mut self, ctx: &QuickAddContext, task: &mut ParsedTask) {
        for at in self.word_starts() {
            if !self.scan[at..].starts_with('@') {
                continue;
            }
            let Some((name, end)) = self.sigil_name(at) else {
                continue;
            };
            let known = ctx.categories.map(|known| {
                known
                    .iter()
                    .find(|category| category.eq_ignore_ascii_case(&name))
                    .cloned()
            });
            let name = match known {
                Some(Some(category)) => category,
                Some(None) => {
                    task.warnings
                        .push(format!("@{name} isn't one of your categories yet"));
                    name
                }
                None => name,
            };
            if !task
                .categories
                .iter()
                .any(|category| category.eq_ignore_ascii_case(&name))
            {
                task.categories.push(name);
            }
            self.claim(at..end, SpanKind::Label);
        }
    }

    /// `p1`–`p4` (D-017), in lower case only, so `P1 incident` stays a
    /// title, as `Tom` does (Q9). The first one counts.
    fn priority(&mut self, task: &mut ParsedTask) {
        for at in self.word_starts() {
            // The input, not the lowercased scan: `P1` isn't a priority.
            let token = self.input.get(at..at + 2).unwrap_or_default();
            let level = match token.as_bytes() {
                [b'p', level @ b'1'..=b'4'] => level - b'0',
                _ => continue,
            };
            if !ends_word(&self.scan, at + 2) {
                continue;
            }
            if task.priority.is_some() {
                task.warnings.push(format!(
                    "{}: only one importance can be given, so it stays in the title",
                    &self.input[at..at + 2]
                ));
                continue;
            }
            task.priority = Some(level);
            task.importance = read_importance(token).ok();
            self.claim(at..at + 2, SpanKind::Priority);
        }
    }

    /// `+myday` or a lone `*`: recognised, so it isn't a title word, but
    /// not applied until rung 7 brings My Day.
    fn my_day(&mut self, task: &mut ParsedTask) {
        for at in self.word_starts() {
            let rest = &self.scan[at..];
            let length = if rest.starts_with("+myday") {
                6
            } else if rest.starts_with('*') {
                1
            } else {
                continue;
            };
            let clean = rest[length..]
                .chars()
                .next()
                .is_none_or(|next| next.is_whitespace() || next == MASK);
            if !clean {
                continue;
            }
            if !task.my_day {
                task.my_day = true;
                task.warnings.push(format!(
                    "{}: My Day arrives in rung 7, so the task isn't added to it",
                    &self.input[at..at + length]
                ));
            }
            self.claim(at..at + length, SpanKind::MyDay);
        }
    }

    /// `every …` (or a lowercase `daily`): the first one counts.
    fn recurrence(&mut self, ctx: &QuickAddContext) -> Option<recurrence::Read> {
        for at in self.word_starts() {
            let rest = &self.scan[at..];
            let daily = rest.starts_with("daily") && self.input[at..].starts_with("daily");
            if !(rest.starts_with("every ") || daily) {
                continue;
            }
            if let Some(read) = recurrence::read_at(&self.scan, at, &ctx.when) {
                self.claim(at..read.until, SpanKind::Recurrence);
                return Some(read);
            }
        }
        None
    }

    /// `!<time or date>`: the reminder. A `!` with nothing readable after
    /// it stays in the title, with a warning.
    fn reminder(&mut self, ctx: &QuickAddContext, task: &mut ParsedTask) -> Option<When> {
        for at in self.word_starts() {
            if !self.scan[at..].starts_with('!') {
                continue;
            }
            match phrase_at(&self.scan, at + 1, &ctx.when) {
                Some((when, end)) => {
                    self.claim(at..end, SpanKind::Reminder);
                    return Some(when);
                }
                None if self.scan[at + 1..]
                    .chars()
                    .next()
                    .is_some_and(char::is_alphanumeric) =>
                {
                    let word_end = self.scan[at..]
                        .find(char::is_whitespace)
                        .map_or(self.scan.len(), |to| at + to);
                    task.warnings.push(format!(
                        "{}: not a time or date, so it stays in the title",
                        &self.input[at..word_end]
                    ));
                }
                None => {}
            }
        }
        None
    }

    /// `start <date>`.
    fn start(&mut self, ctx: &QuickAddContext) -> Option<When> {
        for at in self.word_starts() {
            if !self.scan[at..].starts_with("start ") {
                continue;
            }
            if let Some((when, end)) = phrase_at(&self.scan, at + 6, &ctx.when)
                && when.date().is_some()
            {
                self.claim(at..end, SpanKind::Start);
                return Some(when);
            }
        }
        None
    }

    /// The bare date and time phrases left, in order, with the guards
    /// that keep titles whole.
    fn dates(&mut self, ctx: &QuickAddContext, task: &mut ParsedTask) -> Vec<When> {
        let mut found: Vec<(When, Range<usize>)> = Vec::new();
        let mut past = 0;
        for at in self.word_starts() {
            if at < past {
                continue;
            }
            let Some((when, end)) = phrase_at(&self.scan, at, &ctx.when) else {
                continue;
            };
            if self.guarded(at, end) {
                continue;
            }
            found.push((when, at..end));
            past = end;
        }
        // The first phrase with a date is the due date; a time on its own
        // may join it. Any other is left in the title.
        let dated = found.iter().position(|(when, _)| when.date().is_some());
        let timed = match dated {
            Some(index) if found[index].0.time().is_some() => None,
            _ => found
                .iter()
                .position(|(when, _)| matches!(when, When::Time(_))),
        };
        let mut used = Vec::new();
        for (index, (when, range)) in found.into_iter().enumerate() {
            if Some(index) == dated || Some(index) == timed {
                self.claim(range, SpanKind::Date);
                used.push(when);
            } else {
                task.warnings.push(format!(
                    "\"{}\" reads as a date too, but only the first counts, so it stays in the \
                     title",
                    &self.input[range]
                ));
            }
        }
        used
    }

    /// A phrase at `start..end` that belongs to the title after all:
    /// `Tom`, `tod` and `sat` other than in lower case; a capitalised
    /// one-word phrase before another capitalised word (`Mark Wednesday
    /// Addams`); and one after `the` (`the 9am standup`).
    fn guarded(&self, start: usize, end: usize) -> bool {
        let original = &self.input[start..end];
        let shouted = original
            .split(|ch: char| !ch.is_alphanumeric())
            .any(|word| {
                LOWERCASE_ONLY.contains(&word.to_ascii_lowercase().as_str())
                    && word.chars().any(char::is_uppercase)
            });
        let one_word = !original.contains(char::is_whitespace);
        let next_word = self.input[end..].trim_start();
        let named = one_word
            && original.starts_with(char::is_uppercase)
            && next_word.len() < self.input[end..].len()
            && next_word.starts_with(char::is_uppercase);
        let after_the = self.scan[..start]
            .trim_end()
            .rsplit(char::is_whitespace)
            .next()
            .is_some_and(|word| word == "the");
        shouted || named || after_the
    }

    /// The input less every span (sorted by start), with runs of spaces
    /// made one.
    fn title(&self) -> String {
        let mut kept = String::with_capacity(self.input.len());
        let mut at = 0;
        for span in &self.spans {
            kept.push_str(&self.input[at..span.start]);
            at = span.end;
            if span.kind == SpanKind::Syntax {
                // An escape or a quote mark goes without a trace: `C\#`
                // is `C#`.
            } else if self.input[at..].starts_with([',', '.', ';', ':', '!', '?']) {
                // `on 12 oct, then pack`: the comma rejoins the text before.
                kept.truncate(kept.trim_end().len());
            } else {
                // A space where a span was, so its neighbours don't join.
                kept.push(' ');
            }
        }
        kept.push_str(&self.input[at..]);
        kept.split_whitespace().collect::<Vec<_>>().join(" ")
    }
}

/// The list `typed` names: the one whose name it is, ignoring case, else
/// the only one whose name starts with it.
fn find_list<'l>(lists: &'l [ListRef], typed: &str) -> Result<&'l ListRef, String> {
    let typed = typed.to_lowercase();
    if let Some(list) = lists.iter().find(|list| list.name.to_lowercase() == typed) {
        return Ok(list);
    }
    let matching: Vec<&ListRef> = lists
        .iter()
        .filter(|list| list.name.to_lowercase().starts_with(&typed))
        .collect();
    match matching.as_slice() {
        [list] => Ok(list),
        [] => Err("no list has that name".into()),
        several => {
            let names: Vec<&str> = several.iter().map(|list| list.name.as_str()).collect();
            Err(format!("could be {}", names.join(" or ")))
        }
    }
}

/// Put the passes' findings together: the due date, the first
/// occurrence, the reminder.
fn settle(
    task: &mut ParsedTask,
    ctx: &QuickAddContext,
    repeats: Option<recurrence::Read>,
    reminder: Option<When>,
    start: Option<When>,
    dates: Vec<When>,
) {
    let now = &ctx.when;
    // A flag's due date wins over the text's (D-018), and everything that
    // hangs off the due date follows it.
    let stated = ctx
        .due
        .or_else(|| dates.iter().find_map(|when| when.date()));
    let mut time: Option<NaiveTime> = dates
        .iter()
        .find_map(|when| when.time())
        .or_else(|| repeats.as_ref().and_then(|repeats| repeats.time));
    task.due = match (&repeats, stated, time) {
        (_, Some(date), _) => Some(date),
        (Some(repeats), None, _) => {
            // The next occurrence: today's counts unless its time has gone.
            let from = match time {
                Some(time) => now.next_at(time).map(|at| at.date()),
                None => Some(now.today()),
            };
            from.map(|from| Recurrence::first_on_or_after(&repeats.pattern, from))
        }
        (None, None, Some(time)) => now.next_at(time).map(|at| at.date()),
        (None, None, None) => None,
    };
    if let Some(start) = start {
        task.start = start.date();
        time = time.or(start.time());
        if task.due.is_none() {
            // What Graph does anyway (S11), made visible.
            task.due = task.start;
            if let Some(date) = task.start {
                task.warnings.push(format!(
                    "a start date also sets the due date when there's none, so it's due {}",
                    preview(DueSpec::Date(date), now)
                ));
            }
        }
    }
    if let (Some(repeats), Some(due)) = (repeats, task.due) {
        let end = match repeats.end {
            RecurrenceEnd::Until(until) if until < due => {
                task.warnings.push(format!(
                    "it would end on {}, before it starts, so it has no end",
                    preview(DueSpec::Date(until), now)
                ));
                RecurrenceEnd::Never
            }
            end => end,
        };
        task.recurrence = Some(Recurrence {
            pattern: repeats.pattern,
            end,
            start: due,
        });
    }
    task.reminder = match (reminder, time) {
        (Some(explicit), timed) => {
            if timed.is_some() {
                task.warnings.push(
                    "the ! reminder wins over the time with the date, which is dropped".into(),
                );
            }
            reminder_at(explicit, task.due, now)
        }
        (None, Some(time)) => task.due.map(|due| due.and_time(time)),
        (None, None) => None,
    };
}

/// A `!` reminder: a day alone is 09:00 on it; a time alone is on the due
/// date when there is one, else the next such time.
fn reminder_at(when: When, due: Option<NaiveDate>, now: &ParseContext) -> Option<NaiveDateTime> {
    match when {
        When::DateTime(at) => Some(at),
        When::Date(date) => Some(date.and_time(DEFAULT_REMINDER_TIME)),
        When::Time(time) => match due {
            Some(due) => Some(due.and_time(time)),
            None => now.next_at(time),
        },
    }
}
