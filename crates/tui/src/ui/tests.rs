//! Frames drawn with `TestBackend` for fixed states, as insta snapshots:
//! the sidebar, the task list and the detail pane, the Planned view's
//! groups, a banner, the undo picker, help, the syncing state and ASCII.

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
    // Home is the sidebar's sixth row; "Pay rent" the list's first.
    assert!(!reversed(&terminal, 2, 6));
    assert!(reversed(&terminal, 26, 1));
    app.focus = Pane::Sidebar;
    terminal
        .draw(|frame| super::draw(frame, &app))
        .expect("draw");
    assert!(reversed(&terminal, 2, 6));
    assert!(!reversed(&terminal, 26, 1));
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
    let mut terminal = Terminal::new(TestBackend::new(110, 30)).expect("terminal");
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
        id: "t2".into(),
        title: "Call Sam".into(),
    };
    let confirming = render(&app);
    let last = |frame: &str| frame.lines().last().unwrap_or_default().to_owned();
    insta::assert_snapshot!(format!("{}\n{}", last(&adding), last(&confirming)));
}

#[test]
fn syncing_before_the_first_sync_not_an_empty_list() {
    let mut app = App::new(crate::glyphs::UNICODE, clock());
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
