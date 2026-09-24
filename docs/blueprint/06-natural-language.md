# 06: Natural-language quick add (`crates/nlp`)

Goal: type `Pay rent every 1st #Home p1 !9am` and get a task with the right list, recurrence, importance and reminder, the way Todoist's quick add works. It's **deterministic and has no LLM.** It's a pure crate with no I/O, so it's easy to test.

## Interface

```rust
pub struct ParseContext { now: DateTime<Tz>, tz: Tz, lists: Vec<ListRef>, categories: Vec<String>, locale: Locale /* week start, date order */ }
pub struct ParsedTask { title: String, list: Option<ListRef>, due: Option<DueSpec>, start: Option<DueSpec>,
                        reminder: Option<DateTime<Tz>>, recurrence: Option<PatternedRecurrence>,
                        importance: Option<Importance>, categories: Vec<String>, my_day: bool,
                        spans: Vec<Span> /* byte ranges + kind, for TUI highlighting */, warnings: Vec<String> }
pub trait QuickAddParser { fn parse(&self, input: &str, ctx: &ParseContext) -> ParsedTask; }
```

`DeterministicParser` is the only implementation in v1. The trait is where an optional local-LLM parser can go later (D-016), with no other changes.

## Grammar (v1)

| Token | Meaning | Example |
|---|---|---|
| `#List` or `#"Multi word"` | Target list. Case-insensitive prefix match, which has to be unique. If it doesn't match, it becomes a warning and stays in the title. With no list given, the task goes to "Tasks" (D-022) | `#Home` |
| `@label` | Category. It's created if it's missing, with confirmation in the TUI; the CLI needs `--create-categories` | `@errands` |
| `p1` `p2` `p3` `p4` | Importance: p1 is high, p2 and p3 are normal, p4 is low. Graph only has three levels, so p2 and p3 both map to normal (D-017) | `p1` |
| `!<time or date>` | Reminder | `!9am`, `!tomorrow 8:30` |
| `*` or `+myday` | Add to My Day | `+myday` |
| `start <date>` | `startDateTime` | `start monday` |
| Date and time phrases | Due date (and time) | `tomorrow`, `next fri 5pm`, `in 3 days`, `on 12 oct` |
| `every …` | Recurrence | see below |

Two more rules:

- Anything not recognised stays in the title. Recognised spans are removed from the title, and runs of spaces are collapsed.
- **Escaping:** anything inside quotes stays literal. `\#` and `\@` are literal characters.

## Date and time parsing

Use a library; don't write a custom date parser:

- **[`clockwords`](https://lib.rs/crates/clockwords)** (0.4.0, March 2026, Rust 2024 edition) is the main choice. It **finds date and time phrases inside free text and returns their byte spans.** That's exactly what the TUI's live highlighting needs, and it handles timezones.
- **[`interim`](https://github.com/conradludgate/interim)**, a maintained fork of chrono-english, is a fallback for plain `date -d`-style phrases if clockwords misses them. Pick the UK dialect for "next friday" (the Friday of next week) versus the US meaning (the coming Friday) through the locale setting.
- Evaluate both in spike S8 against a corpus of about 100 real phrases, before settling. If neither handles one type of phrase, add a small rule for it in the parser.

A date without a time sets `dueDateTime` to that date (To Do due dates are effectively dates). A date with a time sets the due date **and** a reminder at that time, which matches how To Do and Todoist behave. There's a setting `nlp.time_sets_reminder` for this, default true.

## Recurrence (custom parser, and why)

None of the date crates parse "every …". We write a small grammar that maps **directly to Graph's `patternedRecurrence`**. That custom code is justified because no library targets that model:

| Input | pattern | range |
|---|---|---|
| `every day`, `daily` | `daily`, interval 1 | `noEnd` from the start date |
| `every 3 days` | `daily`, interval 3 | |
| `every weekday` | `weekly`, days mon–fri | |
| `every mon, wed` | `weekly`, days mon and wed | |
| `every other week`, `every 2 weeks` | `weekly`, interval 2, day of the start date | |
| `every month on the 1st`, `every 1st` | `absoluteMonthly`, dayOfMonth 1 | |
| `every last friday` | `relativeMonthly`, index `last`, days friday | |
| `every year`, `every 12 oct` | `absoluteYearly`, month and day | |
| `… until 31 dec` / `… for 10 times` | | `endDate` / `numbered` |

`firstDayOfWeek` comes from the locale. `recurrenceTimeZone` is the user's timezone. If the input also has a date, it becomes the `range.startDate` and the first due date.

## Agent safety

- `ms-todo tasks add "<text>"` parses by default, because that's what a human means.
- **`--no-parse`** stores the text as the title, exactly as given.
- **Explicit flags always win over parsed values** (`--due`, `--list`, `--importance`, …).
- `--dry-run --json` returns the `ParsedTask` without writing anything, so an agent can check how its text will be read before committing.

The agent skill tells agents to use `--no-parse` plus explicit flags for anything generated (D-018). Otherwise "Email Friday's report" would get a due date of Friday.

## Testing

Table-driven tests with a fixed `now` and timezone, plus `insta` snapshots of `ParsedTask` for the phrase corpus. Add `proptest` checks that parsing never panics and that the title plus the recognised spans cover the whole input.
