//! Read against S8's fixed `now`: Thursday 24 September 2026, 14:00 in
//! London (BST, +01:00).

use chrono::DateTime;
use proptest::prelude::*;

use super::*;
use crate::QuickAddParser;

fn lists() -> Vec<ListRef> {
    [
        "Tasks",
        "Home",
        "Hobbies",
        "Work",
        "Finances",
        "Admin stuff",
        "ms-todo-spike-2026-09-24",
    ]
    .iter()
    .enumerate()
    .map(|(index, name)| ListRef {
        id: format!("L{index}"),
        name: (*name).to_owned(),
    })
    .collect()
}

fn categories() -> Vec<String> {
    vec!["Errands".to_owned(), "Admin".to_owned()]
}

fn now() -> ParseContext {
    ParseContext::new(DateTime::parse_from_rfc3339("2026-09-24T14:00:00+01:00").expect("now"))
}

fn parse(input: &str) -> ParsedTask {
    let lists = lists();
    let categories = categories();
    let ctx = QuickAddContext {
        when: now(),
        lists: &lists,
        categories: Some(&categories),
        due: None,
    };
    DeterministicParser.parse(input, &ctx)
}

/// The due-date phrase found and what it resolves to, as S8's corpus
/// writes them: `-` for none, `YYYY-MM-DD`, or `YYYY-MM-DD HH:MM` when it
/// set a reminder too.
fn date_reading(input: &str) -> (String, String) {
    let parsed = parse(input);
    let phrases: Vec<&str> = parsed
        .spans
        .iter()
        .filter(|span| span.kind == SpanKind::Date)
        .map(|span| &input[span.start..span.end])
        .collect();
    if phrases.is_empty() {
        return ("-".into(), "-".into());
    }
    let value = match (parsed.reminder, parsed.due) {
        (Some(at), _) => at.format("%Y-%m-%d %H:%M").to_string(),
        (None, Some(due)) => due.format("%Y-%m-%d").to_string(),
        (None, None) => "nothing".into(),
    };
    (phrases.join(" "), value)
}

/// S8's rows this build doesn't read as graded, and why. Everything else
/// in the corpus must pass.
const KNOWN_MISSES: &[(&str, &str)] = &[
    (
        "T06",
        "next month is the 1st, not S8's guessed same day (D-045)",
    ),
    ("T10", "US order, M/D/Y (Q8: D/M)"),
    ("T11", "D/M/Y with a year isn't a rule yet"),
    ("T12", "Y/M/D with slashes isn't a rule"),
    ("T14", "a bare ordinal day, 27th, isn't a rule"),
    ("T15", "mid January isn't a rule"),
    ("T17", "at 10 with no am/pm isn't a time"),
    ("T21", "at 1900 isn't a time"),
    ("T27", "in 2 hours: due dates have no time (D-027)"),
    ("T28", "in the morning isn't a rule (Q7)"),
    ("T29", "someday isn't a rule"),
    ("T32", "next weekend isn't a rule"),
    ("T33", "this weekend isn't a rule"),
    ("T34", "next year isn't a rule"),
    ("T35", "3rd friday jan isn't a rule"),
    ("T36", "tom morning: morning isn't a rule (Q7)"),
    ("T37", "tom evening: evening isn't a rule (Q7)"),
    ("T38", "6 weeks before 21 Jul isn't a rule"),
    ("A13", "D/M/YY isn't a rule"),
    ("A14", "dotted dates aren't a rule"),
    ("M06", "eod isn't a rule (Q7)"),
    ("D04", "friday week isn't a rule"),
];

#[test]
fn the_s8_corpus_reads_as_graded_inside_titles() {
    let corpus = include_str!("../../../../docs/research/spikes/S8-corpus.tsv");
    let mut passed = 0;
    let mut total = 0;
    let mut failures = Vec::new();
    for line in corpus.lines().filter(|line| !line.starts_with('#')) {
        let columns: Vec<&str> = line.split('\t').collect();
        let [id, _, input, phrase, expected, ..] = columns.as_slice() else {
            continue;
        };
        total += 1;
        let got = date_reading(input);
        let known = KNOWN_MISSES.iter().any(|(miss, _)| miss == id);
        if got == ((*phrase).to_owned(), (*expected).to_owned()) {
            passed += 1;
            assert!(!known, "{id} passes now: take it off KNOWN_MISSES");
        } else if !known {
            failures.push(format!(
                "{id} {input:?}: got {got:?}, want ({phrase:?}, {expected:?})"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert_eq!(total, 127);
    assert_eq!(passed, total - KNOWN_MISSES.len());
}

/// A reading in one block per input, for the snapshot.
fn render(input: &str) -> String {
    let parsed = parse(input);
    let mut out = format!("{input}\n  title: {:?}\n", parsed.title);
    if let Some(list) = &parsed.list {
        out.push_str(&format!("  list: {}\n", list.name));
    }
    if let Some(due) = parsed.due {
        out.push_str(&format!("  due: {due}\n"));
    }
    if let Some(start) = parsed.start {
        out.push_str(&format!("  start: {start}\n"));
    }
    if let Some(at) = parsed.reminder {
        out.push_str(&format!("  reminder: {}\n", at.format("%Y-%m-%d %H:%M")));
    }
    if let Some(recurrence) = &parsed.recurrence {
        out.push_str(&format!(
            "  recurrence: {} {}\n",
            recurrence.describe(),
            recurrence.to_graph()
        ));
    }
    if let Some(priority) = parsed.priority {
        out.push_str(&format!(
            "  importance: p{priority} {}\n",
            parsed.importance.map_or("?", Importance::name)
        ));
    }
    if !parsed.categories.is_empty() {
        out.push_str(&format!("  categories: {:?}\n", parsed.categories));
    }
    if parsed.my_day {
        out.push_str("  my_day: true\n");
    }
    let spans: Vec<String> = parsed
        .spans
        .iter()
        .map(|span| format!("{}:{:?}", span.kind.name(), &input[span.start..span.end]))
        .collect();
    out.push_str(&format!("  spans: {}\n", spans.join(" ")));
    for warning in &parsed.warnings {
        out.push_str(&format!("  warning: {warning}\n"));
    }
    out.push_str(&format!(
        "  preview: {}\n",
        parsed.summary(&now()).join(" · ")
    ));
    out
}

#[test]
fn representative_inputs() {
    let inputs = [
        "Pay rent every 1st #Finances p1 9am",
        "Call mum in 2 days",
        "Pay rent every 1st #Home p1 !9am",
        "Buy milk #Home @errands p2",
        "Stand-up every weekday 9:30",
        "Call dentist !tomorrow 8:30",
        "Water plants every mon, wed",
        "Gym every other week",
        "#Work Prep deck tomorrow p1",
        "Pay rent every month on the 1st",
        "Call @mum p1 friday",
        "Budget review every last friday",
        "Tax return #\"Admin stuff\" 31 jan",
        "Email Friday's report",
        "Ask Tom about invoice",
        "Call mum tom",
        "Sat nav update",
        "Plan party next sat",
        "Call May about the lease",
        "Renew passport may 12",
        "Mark Wednesday Addams costume",
        "Fix the 9am standup bot",
        "Read \"Next Friday\" by tomorrow",
        "Tweet about \\#rust and \\@ferris",
        "Plan trip start monday",
        "Plan trip start mon due fri",
        "Plan trip start monday fri",
        "Pay invoice #Ho p3",
        "Pay invoice #Nope",
        "Pick up parcel +myday",
        "Pick up parcel * p4",
        "Take vitamins daily 8am",
        "Daily standup notes",
        "Back up laptop every 2 weeks on sun until 31 dec",
        "Water ferns every 3 days for 10 times",
        "Anniversary every 12 oct",
        "Review goals every year",
        "Clean gutters every 3 months",
        "Dentist fri 3pm !fri 9am",
        "Dentist 9am blah tomorrow and fri",
        "Book tickets on 12 oct, then pack",
        "!",
        "every",
        "p1 #Home",
    ];
    let rendered: Vec<String> = inputs.iter().map(|input| render(input)).collect();
    insta::assert_snapshot!(rendered.join("\n"));
}

#[test]
fn recurrences_map_to_graphs_patterned_recurrence() {
    let inputs = [
        "every day",
        "daily",
        "every 3 days",
        "every other day",
        "every weekday",
        "every weekend",
        "every mon, wed",
        "every tue and thu",
        "every week",
        "every other week",
        "every 2 weeks on fri",
        "every month",
        "every month on the 1st",
        "every 1st",
        "every 31st",
        "every 15th of the month",
        "every last friday",
        "every first monday of the month",
        "every 2 months on the last fri",
        "every year",
        "every 29 feb",
        "every oct 12",
        "every day until 30 sep",
        "every week for 4 times",
        "every mon 9am",
        "every day at 7:30 until 1 oct",
    ];
    let rendered: Vec<String> = inputs
        .iter()
        .map(|input| {
            let parsed = parse(&format!("Task {input}"));
            match &parsed.recurrence {
                Some(recurrence) => format!(
                    "{input}\n  {}\n  due {:?} reminder {:?}\n  {}",
                    recurrence.describe(),
                    parsed.due,
                    parsed.reminder,
                    recurrence.to_graph()
                ),
                None => format!("{input}\n  not read: {:?}", parsed.warnings),
            }
        })
        .collect();
    insta::assert_snapshot!(rendered.join("\n"));
}

#[test]
fn the_demo_reads_as_promised() {
    let parsed = parse("Pay rent every 1st #Finances p1 9am");
    assert_eq!(parsed.title, "Pay rent");
    assert_eq!(parsed.list.map(|list| list.name), Some("Finances".into()));
    assert_eq!(parsed.importance, Some(Importance::High));
    assert_eq!(
        parsed.due.map(|due| due.to_string()),
        Some("2026-10-01".into())
    );
    assert_eq!(
        parsed.reminder.map(|at| at.to_string()),
        Some("2026-10-01 09:00:00".into())
    );
    let graph = parsed.recurrence.expect("recurring").to_graph();
    assert_eq!(graph["pattern"]["type"], "absoluteMonthly");
    assert_eq!(graph["pattern"]["dayOfMonth"], 1);
    assert_eq!(graph["range"]["startDate"], "2026-10-01");
    assert_eq!(
        parse("Pay rent every 1st #Finances p1 9am")
            .summary(&now())
            .join(" · "),
        "p1 · due Thu 1 Oct · every month on the 1st · remind 09:00"
    );

    let parsed = parse("Call mum in 2 days");
    assert_eq!(parsed.title, "Call mum");
    assert_eq!(
        parsed.due.map(|due| due.to_string()),
        Some("2026-09-26".into())
    );
    assert_eq!(parsed.reminder, None);
}

#[test]
fn a_recurrence_starts_on_its_next_occurrence_or_the_stated_date() {
    // Thursday 24 September, 14:00.
    for (input, due) in [
        ("x every day", "2026-09-24"),
        // Today's 9am has gone: tomorrow's is the first.
        ("x every day 9am", "2026-09-25"),
        ("x every day 5pm", "2026-09-24"),
        ("x every thu", "2026-09-24"),
        ("x every mon", "2026-09-28"),
        ("x every 1st", "2026-10-01"),
        ("x every 24th", "2026-09-24"),
        ("x every 31st", "2026-10-31"),
        ("x every last fri", "2026-09-25"),
        ("x every first mon", "2026-10-05"),
        ("x every 12 oct", "2026-10-12"),
        ("x every 1 jan", "2027-01-01"),
        ("x every 29 feb", "2028-02-29"),
        ("x every week", "2026-09-24"),
        ("x every mon 12 oct", "2026-10-12"),
    ] {
        assert_eq!(
            parse(input).due.map(|due| due.to_string()).as_deref(),
            Some(due),
            "{input:?}"
        );
    }
}

#[test]
fn a_time_goes_to_the_reminder_on_the_due_date() {
    let parsed = parse("Drinks fri 7pm");
    assert_eq!(
        parsed.due.map(|due| due.to_string()),
        Some("2026-09-25".into())
    );
    assert_eq!(
        parsed.reminder.map(|at| at.to_string()),
        Some("2026-09-25 19:00:00".into())
    );
    // A ! time alone is on the due date when there is one.
    let parsed = parse("Dentist fri !8am");
    assert_eq!(
        parsed.reminder.map(|at| at.to_string()),
        Some("2026-09-25 08:00:00".into())
    );
    // A ! day alone is 09:00 on it, with no due date.
    let parsed = parse("Dentist !tomorrow");
    assert_eq!(parsed.due, None);
    assert_eq!(
        parsed.reminder.map(|at| at.to_string()),
        Some("2026-09-25 09:00:00".into())
    );
}

#[test]
fn a_due_date_given_outside_the_text_wins_and_the_rest_follows_it() {
    let lists = lists();
    let ctx = QuickAddContext {
        when: now(),
        lists: &lists,
        categories: None,
        due: chrono::NaiveDate::from_ymd_opt(2026, 10, 5),
    };
    let parsed = DeterministicParser.parse("Gym every mon tomorrow 7am", &ctx);
    assert_eq!(parsed.title, "Gym");
    assert_eq!(
        parsed.due.map(|due| due.to_string()),
        Some("2026-10-05".into())
    );
    assert_eq!(
        parsed.reminder.map(|at| at.to_string()),
        Some("2026-10-05 07:00:00".into())
    );
    assert_eq!(
        parsed
            .recurrence
            .map(|recurrence| recurrence.start.to_string()),
        Some("2026-10-05".into())
    );
}

#[test]
fn start_also_sets_the_due_date_and_says_so() {
    let parsed = parse("Plan trip start monday");
    assert_eq!(
        parsed.start.map(|day| day.to_string()),
        Some("2026-09-28".into())
    );
    assert_eq!(parsed.due, parsed.start);
    assert!(
        parsed.warnings[0].contains("also sets the due date"),
        "{:?}",
        parsed.warnings
    );
    let parsed = parse("Plan trip start monday fri");
    assert_eq!(
        parsed.due.map(|day| day.to_string()),
        Some("2026-09-25".into())
    );
    assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
}

#[test]
fn lists_match_by_name_or_a_unique_prefix() {
    assert_eq!(parse("x #home").list.map(|list| list.id), Some("L1".into()));
    assert_eq!(parse("x #fin").list.map(|list| list.id), Some("L4".into()));
    assert_eq!(
        parse("x #\"admin stuff\"").list.map(|list| list.id),
        Some("L5".into())
    );
    assert_eq!(
        parse("x #admin").list.map(|list| list.id),
        Some("L5".into())
    );
    let ambiguous = parse("x #ho");
    assert_eq!(ambiguous.list, None);
    assert_eq!(ambiguous.title, "x #ho");
    assert!(ambiguous.warnings[0].contains("Home or Hobbies"));
    // Only the first list counts.
    let two = parse("x #home #work");
    assert_eq!(two.list.map(|list| list.name), Some("Home".into()));
    assert_eq!(two.title, "x #work");
    // Not at a word's start: text.
    assert_eq!(parse("Learn C#").title, "Learn C#");
    assert_eq!(parse("Learn C#").warnings, Vec::<String>::new());
}

#[test]
fn two_lists_with_one_name_are_ambiguous_not_the_first() {
    let mut lists = lists();
    lists.push(ListRef {
        id: "L-home-2".into(),
        name: "home".into(),
    });
    let ctx = QuickAddContext {
        when: now(),
        lists: &lists,
        categories: None,
        due: None,
    };
    for typed in ["x #Home", "x #home"] {
        let parsed = DeterministicParser.parse(typed, &ctx);
        assert_eq!(parsed.list, None, "{typed:?}");
        assert_eq!(parsed.title, typed);
        assert!(
            parsed.warnings[0].contains("2 lists are called Home"),
            "{:?}",
            parsed.warnings
        );
    }
}

#[test]
fn labels_keep_their_known_spelling_and_warn_when_unknown() {
    let parsed = parse("x @errands @new-one @Errands");
    assert_eq!(parsed.categories, ["Errands", "new-one"]);
    assert_eq!(
        parsed.warnings,
        ["@new-one isn't one of your categories yet"]
    );
    assert_eq!(parsed.title, "x");
    // An email address isn't a label.
    assert_eq!(
        parse("mail bob@example.com").categories,
        Vec::<String>::new()
    );
    // Unknown categories aren't called unknown.
    let lists = lists();
    let ctx = QuickAddContext {
        when: now(),
        lists: &lists,
        categories: None,
        due: None,
    };
    assert!(
        DeterministicParser
            .parse("x @new", &ctx)
            .warnings
            .is_empty()
    );
}

#[test]
fn quotes_and_escapes_keep_text_literal() {
    let parsed = parse("Read \"Next Friday\" tomorrow");
    assert_eq!(parsed.title, "Read Next Friday");
    assert_eq!(
        parsed.due.map(|due| due.to_string()),
        Some("2026-09-25".into())
    );
    let parsed = parse("Tweet \\#rust \\@ferris \\!9am");
    assert_eq!(parsed.title, "Tweet #rust @ferris !9am");
    assert_eq!(parsed.list, None);
    assert_eq!(parsed.reminder, None);
    // Inside a word, an escape leaves no space behind.
    assert_eq!(parse("Learn C\\# well").title, "Learn C# well");
    assert_eq!(parse("Say \"x\"y").title, "Say xy");
    // Unclosed: an ordinary character.
    assert_eq!(parse("Say \"hi tomorrow").title, "Say \"hi");
}

#[test]
fn my_day_is_read_out_of_the_title() {
    for input in [
        "Parcel +myday",
        "Parcel *",
        "Parcel +MyDay",
        "+myday Parcel *",
    ] {
        let parsed = parse(input);
        assert!(parsed.my_day, "{input:?}");
        assert_eq!(parsed.title, "Parcel");
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
    }
    assert!(!parse("Rate it *****").my_day);
}

#[test]
fn priorities_count_in_lower_case_only() {
    for input in ["P1 incident review", "P3 review"] {
        let parsed = parse(input);
        assert_eq!(parsed.importance, None, "{input:?}");
        assert_eq!(parsed.title, input);
        assert!(parsed.spans.is_empty(), "{input:?}");
    }
    assert_eq!(
        parse("Incident review p1").importance,
        Some(Importance::High)
    );
}

#[test]
fn only_the_first_date_counts() {
    let parsed = parse("Call bank fri or mon");
    assert_eq!(parsed.title, "Call bank or mon");
    assert_eq!(
        parsed.due.map(|due| due.to_string()),
        Some("2026-09-25".into())
    );
    assert!(
        parsed.warnings[0].contains("\"mon\""),
        "{:?}",
        parsed.warnings
    );
}

#[test]
fn nothing_recognised_is_the_title_as_typed_less_extra_spaces() {
    assert_eq!(parse("  Buy   2 apples  ").title, "Buy 2 apples");
    assert!(parse("Buy 2 apples").spans.is_empty());
    assert_eq!(parse("").title, "");
}

/// The input without its spans, spaces collapsed.
fn uncovered(input: &str, spans: &[Span]) -> String {
    let mut kept = String::new();
    let mut at = 0;
    for span in spans {
        kept.push_str(&input[at..span.start]);
        at = span.end;
        if span.kind == SpanKind::Syntax {
            // Taken out without a trace.
        } else if input[at..].starts_with([',', '.', ';', ':', '!', '?']) {
            kept = kept.trim_end().to_owned();
        } else {
            kept.push(' ');
        }
    }
    kept.push_str(&input[at..]);
    kept.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The spans are in order, apart and on character boundaries, and the
/// title is exactly what they leave.
fn covers(input: &str) -> Result<(), TestCaseError> {
    let parsed = parse(input);
    let mut end = 0;
    for span in &parsed.spans {
        prop_assert!(
            span.start >= end && span.start < span.end,
            "{:?}",
            parsed.spans
        );
        prop_assert!(input.is_char_boundary(span.start) && input.is_char_boundary(span.end));
        end = span.end;
    }
    prop_assert!(end <= input.len());
    prop_assert_eq!(parsed.title, uncovered(input, &parsed.spans));
    Ok(())
}

proptest! {
    #[test]
    fn never_panics_and_the_title_and_spans_cover_the_input(input in ".{0,60}") {
        covers(&input)?;
    }

    #[test]
    fn phrase_like_input_never_panics_either(
        input in "([A-Za-z]{0,6} ){0,3}(every |!|start |#|@|p|\\+myday |\\*|\"|\\\\)?(in |next |this |on |at |\\+)?[0-9]{0,4}( ?(days?|weeks?|months?|am|pm|:[0-9]{0,3}|/[0-9]{0,3}|st|th|other|last|fri|mon|oct|until|for|times))*( [A-Za-z#@\"]{0,6}){0,3}"
    ) {
        let parsed = parse(&input);
        prop_assert_eq!(parsed.title, uncovered(&input, &parsed.spans));
    }
}

#[test]
fn parsing_is_fast_enough_for_every_keystroke() {
    // Warm the compiled rules, as the TUI does on start.
    let _ = parse("warm up tomorrow");
    let input = "Pay rent every 1st #Finances p1 9am @errands start mon !fri 8:30";
    let runs = 200;
    let started = std::time::Instant::now();
    for _ in 0..runs {
        std::hint::black_box(parse(std::hint::black_box(input)));
    }
    let each = started.elapsed() / runs;
    eprintln!("quick add: {each:?} per parse");
    // Debug builds are slow; the budget for a keypress is 16 ms.
    assert!(
        each < std::time::Duration::from_millis(4),
        "{each:?} per parse"
    );
}

#[test]
fn a_list_token_reads_back_as_its_list() {
    assert_eq!(list_token("Finances"), "#Finances");
    assert_eq!(list_token("Admin stuff"), "#\"Admin stuff\"");
    assert_eq!(list_token("Home."), "#\"Home.\"");
    for list in lists() {
        let parsed = parse(&format!("Pay rent {}", list_token(&list.name)));
        assert_eq!(parsed.list.as_ref(), Some(&list), "{}", list.name);
        assert_eq!(parsed.title, "Pay rent");
    }
}

#[test]
fn a_quoted_name_never_takes_in_a_part_already_read() {
    // `#home` inside the quotes is read as the list first; the label's
    // quotes can't then claim it again (they overlapped, and the title
    // panicked).
    let parsed = parse("@\"x #home y\"");
    assert_eq!(
        parsed.list.as_ref().map(|list| list.name.as_str()),
        Some("Home")
    );
    assert!(parsed.categories.is_empty(), "{parsed:?}");
    assert_eq!(parsed.title, "@\"x y\"");
    let parsed = parse("@\"\u{a0}#Home\u{3000}\"");
    assert_eq!(
        parsed.list.as_ref().map(|list| list.name.as_str()),
        Some("Home")
    );
    assert!(parsed.categories.is_empty(), "{parsed:?}");
    // Found by the proptest below.
    covers("#\" @\"\\@\"").expect("covered");
}

proptest! {
    // Rare shapes: at the default 256 cases it missed `@"x #home"`.
    #![proptest_config(ProptestConfig::with_cases(4096))]

    // Dense in what the passes read, and in spaces wider than a byte.
    #[test]
    fn sigils_quotes_and_wide_spaces_never_overlap(
        input in "(@\"|#\"|#home|#w|@x|\"| |\u{a0}|\u{3000}|x|é|\\\\|!9am|p1){0,8}"
    ) {
        covers(&input)?;
    }
}
