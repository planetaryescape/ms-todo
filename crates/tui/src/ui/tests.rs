//! Frames drawn with `TestBackend` for fixed states, as insta snapshots:
//! the sidebar, the task list and the detail pane, the Planned view's
//! groups, a banner, the undo picker, help, the syncing state, ASCII,
//! editing a field, the selection, the palette and the diagnostics page.

use ms_todo_protocol::{Candidate, Scope};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::Modifier;
use serde_json::json;

use crate::app::tests::{clock, home_tasks, seed, seeded, task};
use crate::app::{App, Level, Mode, Msg, Pane};
use crate::glyphs::ASCII;

fn render(app: &App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(110, 16)).expect("terminal");
    terminal
        .draw(|frame| super::draw(frame, app))
        .expect("draw");
    terminal.backend().to_string()
}

#[test]
fn sidebar_list_and_detail() {
    let mut app = seeded();
    app.task_index = 0;
    insta::assert_snapshot!(render(&app));
}

#[test]
fn the_selected_row_is_reversed_in_the_focused_pane_only() {
    let mut app = seeded();
    let mut terminal = Terminal::new(TestBackend::new(110, 16)).expect("terminal");
    let reversed = |terminal: &Terminal<TestBackend>, x: u16, y: u16| {
        terminal.backend().buffer()[(x, y)]
            .modifier
            .contains(Modifier::REVERSED)
    };
    terminal
        .draw(|frame| super::draw(frame, &app))
        .expect("draw");
    // Under the title bar, Home is the sidebar's sixth row; "Pay rent"
    // the list's first.
    assert!(!reversed(&terminal, 2, 7));
    assert!(reversed(&terminal, 26, 2));
    app.focus = Pane::Sidebar;
    terminal
        .draw(|frame| super::draw(frame, &app))
        .expect("draw");
    assert!(reversed(&terminal, 2, 7));
    assert!(!reversed(&terminal, 26, 2));
}

#[test]
fn planned_is_grouped_by_how_soon() {
    let mut app = seeded();
    let due = |date: &str| json!({ "dueDateTime": { "dateTime": format!("{date}T00:00:00.0000000"), "timeZone": "Europe/London" } });
    let tasks = vec![
        task("p1", "Renew passport", due("2026-09-20")),
        task("p2", "Dentist", due("2026-09-24")),
        task("p3", "Bins out", due("2026-09-25")),
        task("p4", "Book train", due("2026-09-27")),
        task("p5", "Pay rent", due("2026-10-01")),
    ];
    let effects = app.update(Msg::Event(ms_todo_protocol::Event::ResyncNeeded));
    app.update(Msg::Response {
        tag: effects[0].tag,
        result: Ok(ms_todo_protocol::ResponseData::Seed(seed(
            Scope::Planned,
            tasks,
        ))),
    });
    app.task_index = 2;
    insta::assert_snapshot!(render(&app));
}

#[test]
fn a_rejection_banner_and_the_failed_row() {
    let mut app = seeded();
    app.task_index = 2;
    app.rejections
        .insert("t4".into(), "the title is too long".into());
    app.show(
        Level::Error,
        "Microsoft To Do rejected a change, which was undone here: the title is too long",
    );
    insta::assert_snapshot!(render(&app));
}

#[test]
fn the_undo_picker() {
    let mut app = seeded();
    app.mode = Mode::Picker {
        target: "op-7".into(),
        candidates: vec![
            Candidate {
                id: "c1".into(),
                name: "Pay rent".into(),
                created_at: Some("2026-09-24T10:00:12.1234567Z".into()),
                list_id: Some("home".into()),
            },
            Candidate {
                id: "c2".into(),
                name: "Pay rent".into(),
                created_at: Some("2026-09-24T10:01:40.1234567Z".into()),
                list_id: Some("home".into()),
            },
        ],
        index: 1,
    };
    insta::assert_snapshot!(render(&app));
}

#[test]
fn help_lists_every_key() {
    let mut app = seeded();
    app.mode = Mode::Help;
    let mut terminal = Terminal::new(TestBackend::new(110, 34)).expect("terminal");
    terminal
        .draw(|frame| super::draw(frame, &app))
        .expect("draw");
    insta::assert_snapshot!(terminal.backend().to_string());
}

#[test]
fn prompts_take_over_the_hint_bar() {
    let mut app = seeded();
    app.mode = Mode::Adding {
        text: "Buy milk".into(),
    };
    let adding = render(&app);
    app.mode = Mode::ConfirmDelete {
        ids: vec!["t2".into()],
        what: "\"Call Sam\"".into(),
    };
    let confirming = render(&app);
    let last = |frame: &str| frame.lines().last().unwrap_or_default().to_owned();
    insta::assert_snapshot!(format!("{}\n{}", last(&adding), last(&confirming)));
}

#[test]
fn syncing_before_the_first_sync_not_an_empty_list() {
    let mut app = App::new(crate::glyphs::UNICODE, clock());
    app.version = "9.9.9";
    app.update(Msg::Connected);
    app.seeded = true;
    app.activity.in_progress = true;
    insta::assert_snapshot!(render(&app));
}

#[test]
fn ascii_glyphs() {
    let mut app = seeded();
    app.glyphs = ASCII;
    let _ = home_tasks();
    insta::assert_snapshot!(render(&app));
}

#[test]
fn editing_a_field_shows_the_text_and_why_it_cant_be_sent() {
    use crate::action::Action;
    use crate::app::edit::Field;
    let mut app = seeded();
    app.update(Msg::Action(Action::FocusRight));
    app.update(Msg::Action(Action::MoveDown));
    // The cursor on Due, not yet editing.
    let cursor = render(&app);
    app.update(Msg::Action(Action::Edit));
    for _ in 0.."2026-10-01".len() {
        app.update(Msg::Action(Action::Backspace));
    }
    for ch in "2026-10-5x".chars() {
        app.update(Msg::Char(ch));
    }
    let typing = render(&app);
    app.update(Msg::Action(Action::Submit));
    assert!(matches!(
        app.mode,
        Mode::Editing {
            field: Field::Due,
            error: Some(_),
            ..
        }
    ));
    insta::assert_snapshot!(format!("{cursor}\n{typing}\n{}", render(&app)));
}

#[test]
fn the_selection_is_marked_and_counted() {
    use crate::action::Action;
    let mut app = seeded();
    app.update(Msg::Action(Action::ToggleSelect));
    app.update(Msg::Action(Action::MoveDown));
    app.update(Msg::Action(Action::MoveDown));
    app.update(Msg::Action(Action::ToggleSelect));
    let selected = render(&app);
    app.update(Msg::Action(Action::Delete));
    let confirming = render(&app);
    let last = |frame: &str| frame.lines().last().unwrap_or_default().to_owned();
    insta::assert_snapshot!(format!("{selected}\n{}", last(&confirming)));
}

#[test]
fn the_palette() {
    use crate::action::Action;
    let mut app = seeded();
    app.update(Msg::Action(Action::Palette));
    let everything = render(&app);
    for ch in "go".chars() {
        app.update(Msg::Char(ch));
    }
    app.update(Msg::Action(Action::MoveDown));
    insta::assert_snapshot!(format!("{everything}\n{}", render(&app)));
}

#[test]
fn the_diagnostics_page() {
    use crate::action::Action;
    let mut app = seeded();
    let effects = app.update(Msg::Action(Action::Diagnostics));
    let checking = render(&app);
    crate::app::diagnostics::tests::answer(&mut app, &effects);
    let mut terminal = Terminal::new(TestBackend::new(110, 26)).expect("terminal");
    terminal
        .draw(|frame| super::draw(frame, &app))
        .expect("draw");
    insta::assert_snapshot!(format!("{checking}\n{}", terminal.backend()));
}

/// j and k in the detail pane move down and up the rows it draws: the
/// reversed row is always the field under the cursor.
#[test]
fn the_detail_cursor_follows_the_rows_on_screen() {
    use crate::app::edit::Field;
    let mut app = seeded();
    app.focus = Pane::Detail;
    let mut terminal = Terminal::new(TestBackend::new(110, 20)).expect("terminal");
    let mut rows = Vec::new();
    for field in Field::ALL {
        app.detail_field = field;
        terminal
            .draw(|frame| super::draw(frame, &app))
            .expect("draw");
        let buffer = terminal.backend().buffer();
        // The detail pane's first column inside its border.
        let x = 77;
        let row = (0..20)
            .find(|&y| buffer[(x, y)].modifier.contains(Modifier::REVERSED))
            .expect("a reversed row");
        let label: String = (x..x + 10).map(|x| buffer[(x, row)].symbol()).collect();
        assert_eq!(label.trim(), field.name(), "{field:?}");
        rows.push(row);
    }
    assert!(rows.is_sorted(), "top to bottom: {rows:?}");
}

/// Titles, notes, list names and banners come from Graph. ratatui drops
/// control characters when it fills the buffer (ratatui-core's
/// `Span::styled_graphemes` and `Buffer::set_stringn`), and the buffer is
/// all the terminal gets, so an escape sequence in them can't reach it.
/// This pins that down against a ratatui upgrade.
#[test]
fn escape_sequences_from_graph_never_reach_the_terminal() {
    let mut app = seeded();
    let evil = "Pay\x1b]52;c;aGk=\x07 rent\u{9b}2J\x7f";
    app.tasks[0].title = evil.into();
    app.tasks[0].body = Some(crate::app::task::Body {
        content: format!("line one\n{evil}"),
        html: false,
    });
    app.lists[1].1 = evil.into();
    app.show(Level::Error, evil);
    let mut screens = vec![render(&app)];
    app.update(Msg::Action(crate::action::Action::Palette));
    screens.push(render(&app));
    let mut terminal = Terminal::new(TestBackend::new(110, 16)).expect("terminal");
    terminal
        .draw(|frame| super::draw(frame, &app))
        .expect("draw");
    let cells: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    screens.push(cells);
    for screen in screens {
        assert!(
            !screen.contains(['\x1b', '\x07', '\u{9b}', '\x7f']),
            "{screen}"
        );
        assert!(screen.contains("rent"));
    }
}
