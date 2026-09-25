//! Quick add through `update`: read on every key, the task sent on
//! Enter, where it goes, `Ctrl-r`, Tab and the categories. Against the
//! test clock: Thursday 24 September 2026, 13:00 in London.

use ms_todo_nlp::SpanKind;
use ms_todo_protocol::{Importance, NewTask, Request, ResponseData, Scope};
use serde_json::json;

use super::Categories;
use crate::action::Action;
use crate::app::tests::{act, seeded};
use crate::app::{App, Effect, Level, Mode, Msg, Tag};

fn typed(app: &mut App, text: &str) {
    for ch in text.chars() {
        app.update(Msg::Char(ch));
    }
}

fn sent(effects: &[Effect]) -> &NewTask {
    match effects {
        [
            Effect {
                request: Request::AddTask { task, .. },
                ..
            },
        ] => task,
        other => unreachable!("{other:?}"),
    }
}

fn parsed(app: &App) -> &ms_todo_nlp::ParsedTask {
    match &app.mode {
        Mode::Adding {
            parsed: Some(parsed),
            ..
        } => parsed,
        other => unreachable!("{other:?}"),
    }
}

/// The categories answer, as Graph gives them.
fn categories(app: &mut App, names: &[&str]) {
    let value: Vec<_> = names
        .iter()
        .map(|name| json!({ "displayName": name }))
        .collect();
    app.update(Msg::Response {
        tag: Tag::Categories,
        result: Ok(ResponseData::Raw {
            body: json!({ "value": value }),
        }),
    });
}

#[test]
fn every_key_reads_the_text_again_and_enter_sends_what_it_read() {
    let mut app = seeded();
    act(&mut app, Action::Add);
    categories(&mut app, &["Bills"]);
    typed(&mut app, "Pay rent every 1st #Tasks p1 9am @bills");
    let reading = parsed(&app);
    assert_eq!(reading.title, "Pay rent");
    let kinds: Vec<SpanKind> = reading.spans.iter().map(|span| span.kind).collect();
    assert_eq!(
        kinds,
        [
            SpanKind::Recurrence,
            SpanKind::List,
            SpanKind::Priority,
            SpanKind::Date,
            SpanKind::Label
        ]
    );
    assert!(reading.warnings.is_empty(), "{:?}", reading.warnings);
    assert_eq!(app.add_target(), "Tasks");

    let effects = act(&mut app, Action::Submit);
    assert_eq!(app.mode, Mode::Normal);
    let task = sent(&effects);
    assert_eq!(task.title, "Pay rent");
    assert_eq!(task.list.as_deref(), Some("tasks"));
    assert_eq!(task.importance, Some(Importance::High));
    assert_eq!(task.due.as_deref(), Some("2026-10-01"));
    assert_eq!(task.reminder.as_deref(), Some("2026-10-01T09:00"));
    assert_eq!(task.categories, ["Bills"]);
    let recurrence = task.recurrence.as_ref().expect("recurring");
    assert_eq!(recurrence["pattern"]["type"], "absoluteMonthly");
    assert_eq!(recurrence["range"]["startDate"], "2026-10-01");
}

#[test]
fn the_list_is_the_typed_one_then_the_one_on_screen_then_tasks() {
    // Home is on screen.
    let mut app = seeded();
    act(&mut app, Action::Add);
    typed(&mut app, "Call mum in 2 days");
    assert_eq!(app.add_target(), "Home");
    let effects = act(&mut app, Action::Submit);
    let task = sent(&effects);
    assert_eq!(task.list.as_deref(), Some("home"));
    assert_eq!(task.title, "Call mum");
    assert_eq!(task.due.as_deref(), Some("2026-09-26"));

    act(&mut app, Action::Add);
    typed(&mut app, "Call mum #tasks");
    assert_eq!(app.add_target(), "Tasks");
    assert_eq!(
        sent(&act(&mut app, Action::Submit)).list.as_deref(),
        Some("tasks")
    );

    // A view: Tasks, with what puts the task in the view unless the text
    // says otherwise.
    app.shown = Some(Scope::Important);
    app.wanted = app.shown.clone();
    act(&mut app, Action::Add);
    typed(&mut app, "Low one p4");
    assert_eq!(app.add_target(), "Tasks");
    let effects = act(&mut app, Action::Submit);
    let task = sent(&effects);
    assert_eq!(
        (task.list.as_deref(), task.importance),
        (None, Some(Importance::Low))
    );

    app.shown = Some(Scope::Planned);
    app.wanted = app.shown.clone();
    act(&mut app, Action::Add);
    typed(&mut app, "Later fri");
    assert_eq!(
        sent(&act(&mut app, Action::Submit)).due.as_deref(),
        Some("2026-09-25")
    );
}

#[test]
fn ctrl_r_takes_the_text_literally_and_back() {
    let mut app = seeded();
    act(&mut app, Action::Add);
    typed(&mut app, "Email Friday report tomorrow");
    assert!(parsed(&app).due.is_some());
    act(&mut app, Action::ToggleParse);
    assert!(matches!(app.mode, Mode::Adding { parsed: None, .. }));
    act(&mut app, Action::ToggleParse);
    assert!(parsed(&app).due.is_some());
    act(&mut app, Action::ToggleParse);
    let effects = act(&mut app, Action::Submit);
    let task = sent(&effects);
    assert_eq!(task.title, "Email Friday report tomorrow");
    assert_eq!(task.due, None);
}

#[test]
fn nothing_left_for_the_title_keeps_the_modal_open() {
    let mut app = seeded();
    act(&mut app, Action::Add);
    typed(&mut app, "tomorrow p1");
    assert!(act(&mut app, Action::Submit).is_empty());
    assert!(matches!(app.mode, Mode::Adding { .. }));
    assert_eq!(
        app.banner.as_ref().map(|banner| banner.level),
        Some(Level::Error)
    );
}

#[test]
fn tab_completes_a_list_or_a_label() {
    let mut app = seeded();
    act(&mut app, Action::Add);
    categories(&mut app, &["Errands", "Admin"]);
    typed(&mut app, "Buy milk #ho");
    act(&mut app, Action::Complete);
    let Mode::Adding { input, .. } = &app.mode else {
        unreachable!();
    };
    assert_eq!(input.text(), "Buy milk #Home ");
    assert_eq!(
        parsed(&app).list.as_ref().map(|list| list.id.as_str()),
        Some("home")
    );
    typed(&mut app, "@er");
    act(&mut app, Action::Complete);
    let Mode::Adding { input, .. } = &app.mode else {
        unreachable!();
    };
    assert_eq!(input.text(), "Buy milk #Home @Errands ");
    // Nothing to finish: nothing changes.
    typed(&mut app, "x");
    act(&mut app, Action::Complete);
    let Mode::Adding { input, .. } = &app.mode else {
        unreachable!();
    };
    assert_eq!(input.text(), "Buy milk #Home @Errands x");
}

#[test]
fn tab_after_a_wide_space_completes_without_splitting_a_character() {
    for space in ['\u{3000}', '\u{a0}'] {
        let mut app = seeded();
        act(&mut app, Action::Add);
        categories(&mut app, &["Errands"]);
        typed(&mut app, &format!("Buy{space}#ho"));
        act(&mut app, Action::Complete);
        let Mode::Adding { input, .. } = &app.mode else {
            unreachable!();
        };
        assert_eq!(input.text(), format!("Buy{space}#Home "));
        typed(&mut app, &format!("x{space}@er"));
        act(&mut app, Action::Complete);
        let Mode::Adding { input, .. } = &app.mode else {
            unreachable!();
        };
        assert_eq!(input.text(), format!("Buy{space}#Home x{space}@Errands "));
    }
}

proptest::proptest! {
    #[test]
    fn tab_never_panics(text in "[a-z#@\"\u{3000}\u{a0} é]{0,16}") {
        let mut app = seeded();
        act(&mut app, Action::Add);
        typed(&mut app, &text);
        act(&mut app, Action::Complete);
    }
}

#[test]
fn categories_are_asked_for_once_and_a_failure_calls_no_label_unknown() {
    let mut app = seeded();
    assert_eq!(act(&mut app, Action::Add).len(), 1);
    assert_eq!(app.categories, Categories::Asking);
    act(&mut app, Action::Cancel);
    assert!(act(&mut app, Action::Add).is_empty(), "still asking");
    app.update(Msg::Response {
        tag: Tag::Categories,
        result: Err(ms_todo_protocol::ErrorPayload::default()),
    });
    assert_eq!(app.categories, Categories::Known(None));
    typed(&mut app, "x @new");
    assert!(parsed(&app).warnings.is_empty());
    act(&mut app, Action::Cancel);
    assert!(act(&mut app, Action::Add).is_empty());
}

#[test]
fn a_keypress_reads_well_within_a_frame() {
    let mut app = seeded();
    act(&mut app, Action::Add);
    let text = "Pay rent every 1st #Home p1 9am @bills start mon !fri 8:30 and more words";
    let started = std::time::Instant::now();
    typed(&mut app, text);
    let each = started.elapsed() / u32::try_from(text.len()).unwrap_or(1);
    eprintln!("quick add in the TUI: {each:?} per key");
    // The budget is 16 ms a frame; debug builds are slow, so a margin.
    assert!(
        each < std::time::Duration::from_millis(8),
        "{each:?} per key"
    );
}
