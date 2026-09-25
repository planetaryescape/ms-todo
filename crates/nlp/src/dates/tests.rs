//! Read against S8's fixed `now`: Thursday 24 September 2026, 14:00 in
//! London (BST, +01:00).

use chrono::{DateTime, NaiveDate};
use proptest::prelude::*;

use super::*;

fn ctx() -> ParseContext {
    ctx_at("2026-09-24T14:00:00+01:00")
}

fn ctx_at(now: &str) -> ParseContext {
    ParseContext::new(DateTime::parse_from_rfc3339(now).expect("now"))
}

/// A phrase read as S8's corpus writes values: `YYYY-MM-DD`, or
/// `YYYY-MM-DD HH:MM` with a time.
fn when(input: &str) -> String {
    match read_when(input, &ctx()) {
        Ok(Reading::Set {
            value: DueSpec::Date(date),
            ..
        }) => date.format("%Y-%m-%d").to_string(),
        Ok(Reading::Set {
            value: DueSpec::DateTime(at),
            ..
        }) => at.format("%Y-%m-%d %H:%M").to_string(),
        Ok(Reading::Clear) => "clear".into(),
        Err(why) => format!("error: {why}"),
    }
}

fn date(value: &str) -> NaiveDate {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").expect("date")
}

/// The rows of S8's corpus whose phrase is a whole date field's worth,
/// read as the corpus expects. Left out: phrases this build doesn't read
/// (`27th`, `mid January`, `someday`, `in 2 hours`, `eod`, `morning`,
/// `friday week`, `3rd friday jan`, US and dotted dates, `at 1900`,
/// `today at 10`) and T06, where S8 guessed `next month` is the same day
/// next month; this build reads it as the 1st, as asked.
const CORPUS_ROWS: &[&str] = &[
    "T01", "T02", "T03", "T04", "T05", "T07", "T08", "T09", "T13", "T16", "T18", "T19", "T20",
    "T22", "T23", "T24", "T25", "T26", "T30", "T31", "R01", "R02", "R03", "R04", "R05", "R06",
    "R07", "R08", "R09", "R10", "R11", "R12", "R13", "R14", "A01", "A02", "A03", "A04", "A05",
    "A06", "A07", "A08", "A09", "A10", "A11", "A12", "M01", "M02", "M03", "M04", "M05", "M08",
    "M09", "M10", "M11", "C01", "C02", "C03", "C04", "C05", "C06", "C07", "C08", "C09", "C10",
    "C11", "C12", "C13", "D01", "D02", "D03",
];

#[test]
fn the_s8_corpus_phrases_read_as_graded() {
    let corpus = include_str!("../../../../docs/research/spikes/S8-corpus.tsv");
    let mut checked = 0;
    for line in corpus.lines().filter(|line| !line.starts_with('#')) {
        let columns: Vec<&str> = line.split('\t').collect();
        let [id, _, _, phrase, expected, ..] = columns.as_slice() else {
            continue;
        };
        if CORPUS_ROWS.contains(id) {
            assert_eq!(when(phrase), *expected, "{id} {phrase:?}");
            checked += 1;
        }
    }
    assert_eq!(checked, CORPUS_ROWS.len());
}

#[test]
fn titles_with_date_like_words_are_not_dates() {
    // S8's embedded rows: whole-string mode reads none of them.
    for title in [
        "Email Friday's report",
        "Monitor the build",
        "Buy 2 apples",
        "Satisfy the auditor",
        "Call May about the lease",
        "Send the 3 day notice",
        "Ask Tom about invoice",
        "Sat nav update",
        "Last week's receipts",
        "Octopus",
        "12",
    ] {
        assert!(
            when(title).starts_with("error"),
            "{title:?}: {}",
            when(title)
        );
    }
}

#[test]
fn the_phrases_bk_asked_for() {
    for (input, expected) in [
        ("today", "2026-09-24"),
        ("TOMORROW", "2026-09-25"),
        ("  tmrw ", "2026-09-25"),
        ("yesterday", "2026-09-23"),
        ("day before yesterday", "2026-09-22"),
        ("tonight", "2026-09-24"),
        ("three days from today", "2026-09-27"),
        ("Twenty days from now", "2026-10-14"),
        ("2 weeks from today", "2026-10-08"),
        ("in two months", "2026-11-24"),
        ("+3d", "2026-09-27"),
        ("+2w", "2026-10-08"),
        ("+1m", "2026-10-24"),
        ("-1d", "2026-09-23"),
        ("3 days ago", "2026-09-21"),
        ("a week ago", "2026-09-17"),
        ("this thursday", "2026-09-24"),
        ("this mon", "2026-09-28"),
        ("next thu", "2026-10-01"),
        ("next week", "2026-09-28"),
        ("eow", "2026-09-25"),
        ("end of week", "2026-09-25"),
        ("eom", "2026-09-30"),
        ("next month", "2026-10-01"),
        ("2026-10-02", "2026-10-02"),
        ("12/10", "2026-10-12"),
        ("1/9", "2027-09-01"),
        ("oct 12", "2026-10-12"),
        ("noon", "2026-09-25 12:00"),
        ("5:30pm", "2026-09-24 17:30"),
        ("tomorrow 9am", "2026-09-25 09:00"),
        ("fri 17:30", "2026-09-25 17:30"),
        ("2026-10-01 09:30", "2026-10-01 09:30"),
        ("2026-10-01T09:30", "2026-10-01 09:30"),
        ("12am", "2026-09-25 00:00"),
    ] {
        assert_eq!(when(input), expected, "{input:?}");
    }
}

#[test]
fn a_weekday_that_is_today_means_next_week() {
    // Thursday.
    assert_eq!(when("thursday"), "2026-10-01");
    assert_eq!(when("thu"), "2026-10-01");
    assert_eq!(when("fri"), "2026-09-25");
    assert_eq!(when("wed"), "2026-09-30");
}

#[test]
fn a_time_already_past_today_is_tomorrow() {
    assert_eq!(when("09:00"), "2026-09-25 09:00");
    // Exactly now has passed too: a reminder for now would never ring.
    assert_eq!(when("14:00"), "2026-09-25 14:00");
    assert_eq!(when("14:01"), "2026-09-24 14:01");
    // Late on 31 December, the next 9am is next year's.
    let late = ctx_at("2026-12-31T23:30:00+00:00");
    assert_eq!(
        read_reminder("9am", &late),
        Ok(Reading::Set {
            value: date("2027-01-01").and_hms_opt(9, 0, 0).expect("time"),
            preview: "Fri 1 Jan 2027 09:00".into(),
        })
    );
}

#[test]
fn empty_or_a_dash_clears() {
    for input in ["", "   ", "-", " - "] {
        assert_eq!(read_when(input, &ctx()), Ok(Reading::Clear), "{input:?}");
        assert_eq!(read_due(input, &ctx()), Ok(Reading::Clear));
        assert_eq!(read_reminder(input, &ctx()), Ok(Reading::Clear));
    }
}

#[test]
fn the_preview_names_the_day_and_says_when_it_is_past() {
    let preview = |input| match read_when(input, &ctx()) {
        Ok(Reading::Set { preview, .. }) => preview,
        other => format!("{other:?}"),
    };
    assert_eq!(preview("next fri"), "Fri 2 Oct");
    assert_eq!(preview("yesterday"), "Wed 23 Sep, in the past");
    assert_eq!(preview("1 jan"), "Fri 1 Jan 2027");
    assert_eq!(preview("tomorrow 9am"), "Fri 25 Sep 09:00");
    assert_eq!(preview("today 9am"), "Thu 24 Sep 09:00, in the past");
}

#[test]
fn a_due_date_refuses_a_time() {
    assert_eq!(
        read_due("tomorrow", &ctx()),
        Ok(Reading::Set {
            value: date("2026-09-25"),
            preview: "Fri 25 Sep".into()
        })
    );
    let timed = read_due("tomorrow 9am", &ctx()).expect_err("a time");
    assert!(timed.0.contains("reminder"), "{timed}");
    assert!(matches!(
        read_reminder("fri 17:30", &ctx()),
        Ok(Reading::Set { .. })
    ));
}

/// A reminder read against the test clock, as `YYYY-MM-DD HH:MM` and
/// its preview.
fn reminder_at(input: &str, ctx: &ParseContext) -> (String, String) {
    match read_reminder(input, ctx) {
        Ok(Reading::Set { value, preview }) => {
            (value.format("%Y-%m-%d %H:%M").to_string(), preview)
        }
        other => (format!("{other:?}"), String::new()),
    }
}

#[test]
fn a_reminder_with_only_a_day_is_at_nine() {
    // Thursday 24 September, 14:00.
    for (input, at, preview) in [
        ("tomorrow", "2026-09-25 09:00", "Fri 25 Sep 09:00"),
        ("in 2 days", "2026-09-26 09:00", "Sat 26 Sep 09:00"),
        ("fri", "2026-09-25 09:00", "Fri 25 Sep 09:00"),
        ("12 oct", "2026-10-12 09:00", "Mon 12 Oct 09:00"),
        ("next week", "2026-09-28 09:00", "Mon 28 Sep 09:00"),
    ] {
        assert_eq!(
            reminder_at(input, &ctx()),
            (at.to_owned(), preview.to_owned()),
            "{input:?}"
        );
    }
}

#[test]
fn a_reminder_for_today_after_nine_stays_today_and_says_it_is_past() {
    // 14:00: today's 09:00 has gone, and isn't moved to tomorrow.
    assert_eq!(
        reminder_at("today", &ctx()),
        (
            "2026-09-24 09:00".to_owned(),
            "Thu 24 Sep 09:00, in the past".to_owned()
        )
    );
    // 08:00: still to come.
    let early = ctx_at("2026-09-24T08:00:00+01:00");
    assert_eq!(
        reminder_at("today", &early),
        ("2026-09-24 09:00".to_owned(), "Thu 24 Sep 09:00".to_owned())
    );
    // A typed time still wins over the default.
    assert_eq!(reminder_at("tomorrow 17:30", &ctx()).0, "2026-09-25 17:30");
}

#[test]
fn errors_name_what_was_not_understood() {
    assert_eq!(when("tomorrow blah"), "error: didn't understand \"blah\"");
    assert_eq!(when("soonish"), "error: didn't understand \"soonish\"");
    assert_eq!(when("31/02"), "error: \"31/02\" isn't a real date or time");
    assert_eq!(
        when("2026-13-40"),
        "error: \"2026-13-40\" isn't a real date or time"
    );
    assert!(when("25:00").starts_with("error"));
    assert!(when("13pm").starts_with("error"));
    assert!(when("in 99999999999 days").starts_with("error"));
}

/// A day read looking back, as `YYYY-MM-DD`, from Thursday 24 September.
fn since(input: &str) -> String {
    match read_past_date(input, &ctx()) {
        Ok(date) => date.format("%Y-%m-%d").to_string(),
        Err(why) => format!("error: {why}"),
    }
}

#[test]
fn looking_back_a_weekday_or_a_day_with_no_year_is_the_latest_one() {
    for (input, expected) in [
        ("yesterday", "2026-09-23"),
        ("today", "2026-09-24"),
        // Thursday: today counts, and Monday is this week's.
        ("thu", "2026-09-24"),
        ("mon", "2026-09-21"),
        ("fri", "2026-09-18"),
        ("this week", "2026-09-21"),
        ("last week", "2026-09-14"),
        ("this month", "2026-09-01"),
        ("last month", "2026-08-01"),
        ("12 sep", "2026-09-12"),
        ("24 sep", "2026-09-24"),
        // Still to come this year: last year's.
        ("12 oct", "2025-10-12"),
        ("12/10", "2025-10-12"),
        ("3 days ago", "2026-09-21"),
        ("-1w", "2026-09-17"),
        ("2026-09-01", "2026-09-01"),
        ("12 oct 2026", "2026-10-12"),
    ] {
        assert_eq!(since(input), expected, "{input:?}");
    }
    // Ahead, the same words keep their meaning.
    assert_eq!(when("mon"), "2026-09-28");
    assert_eq!(when("12 oct"), "2026-10-12");
    assert_eq!(when("last week"), "2026-09-14");
}

#[test]
fn looking_back_wants_a_day_and_nothing_else() {
    assert!(since("yesterday 9am").contains("without a time"));
    assert!(since("").starts_with("error"));
    assert!(since("-").starts_with("error"));
    assert_eq!(since("soonish"), "error: didn't understand \"soonish\"");
    assert!(when("Last week's receipts").starts_with("error"));
}

proptest! {
    #[test]
    fn never_panics(input in ".{0,40}") {
        let _ = read_when(&input, &ctx());
    }

    #[test]
    fn never_panics_on_phrase_like_input(
        input in "(in |next |this |\\+|-)?[0-9]{0,12}( ?(days?|weeks?|months?|d|w|m|am|pm|:[0-9]{0,3}|/[0-9]{0,3}))?( (mon|fri|oct|at|ago|from now))?"
    ) {
        let _ = read_when(&input, &ctx());
    }
}
