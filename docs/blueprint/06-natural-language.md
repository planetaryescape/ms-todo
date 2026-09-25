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

**As built (rung 6a, D-052).** The shapes differ from the sketch above where the rest of the build had already decided:

```rust
pub struct QuickAddContext<'a> { when: ParseContext /* now, FixedOffset, D-045 */, lists: &'a [ListRef],
                                 categories: Option<&'a [String]> /* None: couldn't be read, so none is called unknown */ }
pub struct ParsedTask { title: String, list: Option<ListRef>, due: Option<NaiveDate>, start: Option<NaiveDate>,
                        reminder: Option<NaiveDateTime>, recurrence: Option<Recurrence>, importance: Option<Importance>,
                        priority: Option<u8> /* the p-level typed */, categories: Vec<String>, my_day: bool,
                        spans: Vec<Span> /* start, end (bytes), kind */, warnings: Vec<String> }
pub trait QuickAddParser { fn parse(&self, input: &str, ctx: &QuickAddContext) -> ParsedTask; }
```

Due and start are dates (D-027), so they're `NaiveDate`, not `DueSpec`. `ParsedTask::summary` gives the preview's parts and `to_json` the CLI's shape. A `Recurrence` is a `Pattern` (daily, weekly, absolute or relative monthly, absolute yearly), a `RecurrenceEnd` (never, until, count) and its `start`; `to_graph()` is Graph's `patternedRecurrence` without the zone, and `describe()` reads it back. Locale isn't in the context yet: weeks start on Monday and dates are D/M (Q8's placeholder).

## Grammar (v1)

| Token | Meaning | Example |
|---|---|---|
| `#List` or `#"Multi word"` | Target list. Case-insensitive prefix match, which has to be unique. If it doesn't match, it becomes a warning and stays in the title. With no list given, the task goes to "Tasks" (D-022) | `#Home` |
| `@label` | Category. It's created if it's missing, with confirmation in the TUI; the CLI needs `--create-categories`. Graph and Outlook show categories; the iOS To Do app doesn't (S7) | `@errands` |
| `p1` `p2` `p3` `p4` | Importance: p1 is high, p2 and p3 are normal, p4 is low. Graph only has three levels, so p2 and p3 both map to normal (D-017) | `p1` |
| `!<time or date>` | Reminder | `!9am`, `!tomorrow 8:30` |
| `*` or `+myday` | Add to My Day | `+myday` |
| `start <date>` | `startDateTime`. With no due date, Graph sets the due date to the start date too (S11), so the parser adds a warning saying so | `start monday` |
| Date and time phrases | Due date. A time goes to the reminder (see below) | `tomorrow`, `next fri 5pm`, `in 3 days`, `on 12 oct` |
| `every …` | Recurrence | see below |

Two more rules:

- Anything not recognised stays in the title. Recognised spans are removed from the title, and runs of spaces are collapsed.
- **Escaping:** anything inside quotes stays literal. `\#` and `\@` are literal characters.

## Date and time parsing

**A custom rule-table scanner in `crates/nlp`** (D-026). This replaces the earlier plan to use `clockwords` with `interim` as a fallback. Spike S8 ran both, plus `chrono-english` and `whichtime-sys`, over 127 graded phrases, and none came close ([S8 evidence](../research/spikes/S8.md)):

- `clockwords` (33%) has no absolute dates (`12 oct`, `27/1`) and no bare or short weekdays (`friday`, `wed`).
- `interim` (24–39%) and `chrono-english` (35%) parse whole strings, so they give no spans for highlighting. They match any word on its first three letters (`Monitor` → Monday, `Octopus` → October), and they don't roll past dates forward.
- `whichtime-sys` (53%) needs `chrono::Local`, reads weekdays towards the past, and still takes `Sat nav` as Saturday.

Custom code is justified here because the job is narrow and the crates fail at its core: finding a date inside a title without eating the title.

**Shipped early: whole-string mode** (D-045). The editing fix built the rule table's first slice in `crates/nlp` (`src/dates/`): one regex per phrase shape plus a resolver (`rules.rs`), exact word tables (`words.rs`), and whole-string mode (`whole_string.rs`), where the input must be a date phrase from end to end, for the `--due` and `--reminder` flags and the TUI's date fields. `ParseContext` carries `now` as a `DateTime<FixedOffset>` in the user's zone (chrono-tz can replace it when quick add needs zone rules, such as `in 2 hours` across a DST change). It reads 71 of S8's corpus phrases as graded; the ones it doesn't are listed in its tests (`27th`, `mid January`, `someday`, `in 2 hours`, `eod`, `morning`, `friday week`, `3rd friday jan`, US and dotted dates, `at 1900`, `today at 10`). One grading differs on purpose: `next month` is the 1st of next month, not S8's same day next month (S8 flagged its value as a guess). Placeholders applied: Q6 (a weekday that is today means next week), Q7 (`tonight` is today with no time), Q8 (D/M). Q9's lowercase-only `tom` is for span mode; whole-string mode lowercases everything. `ms_todo_nlp::read_importance` has D-017's `p1`–`p4` mapping. Rung 6 adds span mode over the same patterns, with the masking passes and guards below.

**Span mode, as built (6a).** `dates/span.rs` runs the same rule regexes at each word start of the title (lowercased ASCII-only, so byte offsets are the input's), longest match wins, and a date may take a time after it (`fri 5pm`, `fri at 5pm`) or a time a date (`9am tomorrow`), with a leading `on` or `at` in the span. A phrase must end a word: the end, a space, a claimed byte, or punctuation that ends the word (`fri, then`); so `Friday's` and `12/10/26` are text. Masking replaces claimed bytes with a mark that starts a new word, and quoted or escaped bytes with one that doesn't, so `\!9am` stays text. The passes, in order: quotes and escapes (`"…"`, and `\#`, `\@`, `\!`, `\*`, `\+`, `\"`, `\\`); `#List` or `#"Two words"` (the first that names a list counts; an unknown or ambiguous one, including a name several lists share, stays in the title with a warning naming the candidates); `@label` (every one); `p1`–`p4` (lower case only, so `P1 incident` stays a title; the first counts); `+myday` or a lone `*` (the task goes in today's My Day, rung 7; no warning); `every …` or a lowercase `daily`; `!` with a phrase; `start ` with a phrase; then the bare phrases, with three more guards: `tom`, `tod` and `sat` in lower case only (Q9), a capitalised one-word phrase before another capitalised word is a name (`Mark Wednesday Addams`), and a phrase after `the` is an adjective (`the 9am standup`). The first phrase with a date is the due date, and a time-only phrase may join it; any other phrase stays in the title with a warning. Removing a span leaves a space (an escape or a quote mark leaves nothing, so `C\#` is `C#`), runs of spaces become one, and a comma after a span rejoins the text before it (`on 12 oct, then pack` → `…, then pack`).

**Settling the fields, as built.** A due date given outside the text (`QuickAddContext.due`, the CLI's `--due`) is the stated date, over the text's, so the reminder and a recurrence's start follow the flag. With a recurrence, the first due date is the stated date, else the pattern's next day from today (tomorrow when the typed time has gone today). With no recurrence, a date is the due date, and a time alone is its next occurrence. A time from the date phrase (or after `every …`) becomes the reminder on the due date. A `!` reminder wins over that time (with a warning); a `!` day alone is 09:00 on it, and a `!` time alone is on the due date if there is one, else its next occurrence. `start <date>` with no due date also sets the due date, as Graph would (S11), with a warning. An `until` before the start is dropped, with a warning.

**How it works:**

- **Ordered passes with masking.** Quoted text and escapes first, then `#List`, `@label`, `p1`–`p4`, `every …`, `!` reminders and `start`, and only then bare date and time phrases. Each pass masks the bytes it claimed, so later passes can't reinterpret them. That's what stops `every mon` or `!9am` from also becoming a due date.
- **Exact word tables**, not prefix matching, so `Monitor` and `Sat nav` stay in the title. A possessive guard leaves `Friday's report` alone.
- **Rolls forward.** A time already past today means tomorrow, and a day and month already past mean next year.
- **Spans for every match**, for the TUI's live highlighting.
- The same scanner reads quick add, the `!` token, `start`, and the `--due`, `--start` and `--reminder` flags, so every surface reads dates the same way.
- Locale settings (D/M or M/D, UK or US "next friday") come from `ParseContext`. Some phrase meanings are still product questions for BK (Q6–Q10 in [12](12-open-questions.md#product-questions-for-bk)).

**Due and start dates have no time** (S11, D-027). Graph keeps only the date. So:

- A date without a time sets the due date.
- **A date with a time sets the due date and a reminder at that time** (`isReminderOn: true`). There's no other place to keep a time, so this isn't a setting; the old `nlp.time_sets_reminder` is gone.
- A time with no date (`5pm`) means the next occurrence of that time, as the due date plus a reminder.

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

As built, the grammar also takes `every weekend` (Saturday and Sunday), `every mon and thu`, `every 2 weeks on fri`, `every month on the last fri`, `every 15th of the month`, `every oct 12`, spelled counts (`every three days`), a lowercase `daily`, and a time after any of them (`every mon 9am`, the first occurrence's reminder). `every week`, `every month` and `every year` take their day from the start date. `firstDayOfWeek` is Monday.

`firstDayOfWeek` comes from the locale. **Always send `range.recurrenceTimeZone`**, set to the user's timezone, the same zone as the due date. Leaving it out moved the due date a day later in S12. If the input also has a date, it becomes the `range.startDate` and the first due date.

## Agent safety

- `ms-todo tasks add "<text>"` parses by default, because that's what a human means.
- **`--no-parse`** stores the text as the title, exactly as given.
- **Explicit flags always win over parsed values** (`--due`, `--list`, `--importance`, …).
- `--dry-run --json` returns the `ParsedTask` without writing anything, so an agent can check how its text will be read before committing.

The agent skill tells agents to use `--no-parse` plus explicit flags for anything generated (D-018). Otherwise "Email Friday's report" would get a due date of Friday. (As built, the possessive guard keeps that one whole, but `Email report Friday` would still be read, which is the point of the flag.)

## List suggestions (rung 6b)

As built (D-053). Quick add stays deterministic; which list a task *might* belong in is a separate, optional question, answered by TypeSafe's Jev model through the daemon, never by `crates/nlp`. It's asked only for a task headed for the inbox (no `#List`, no `--list`, not added from a list's view), and only when `[suggest] enabled = true` in config.toml. It never files anything: the TUI offers `#List` for `Ctrl-l` to type in, and the CLI prints a `note:` and has `tasks suggest-list`. The options are the lists that are filing targets, each described by its folder, name and up to five open task titles; a forced choice, shown only at confidence ≥ `min_confidence` (0.8, where the calibration was right 7 times in 8). What's sent, the failure rules and the key are in D-053.

## Testing

Table-driven tests with a fixed `now` and timezone, plus `insta` snapshots of `ParsedTask` for the phrase corpus. Start from S8's corpus, [S8-corpus.tsv](../research/spikes/S8-corpus.tsv). Add `proptest` checks that parsing never panics and that the title plus the recognised spans cover the whole input.

As built (6a): every one of S8's 127 rows is parsed as a quick-add title with Thursday 24 September 2026 14:00 London as now; 105 read as graded, and the 22 that don't are listed in the test with the reason for each (US and dotted dates, `27th`, `mid January`, `someday`, `in 2 hours`, `eod`, `morning`, `evening`, `next weekend`, `next year`, `friday week`, `3rd friday jan`, `6 weeks before 21 Jul`, `at 1900`, `today at 10`, and `next month` as the 1st), so one that starts passing has to come off the list. 44 representative inputs and 26 recurrences are `insta` snapshots. The proptest property is exact: the title equals the input with each span replaced by a space, runs of spaces collapsed (and a comma rejoined, as above). A parse takes about 20 µs in a release build.
