//! Frames drawn with `TestBackend` for fixed states, as insta snapshots:
//! the sidebar, the task list and the detail pane, the Planned view's
//! groups, a banner, the undo picker, help, the syncing state, ASCII,
//! editing a field (the picker, the cursor, notes, a resolved date), the
//! selection, the palette and the diagnostics page.

use ms_todo_protocol::{Candidate, Scope};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::Modifier;
use serde_json::json;

use crate::app::tests::{answer_seed, clock, home_tasks, seed, seeded, task};
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

/// Whether the cell at `x`, `y` has the theme's selection background.
fn selected(terminal: &Terminal<TestBackend>, app: &App, x: u16, y: u16) -> bool {
    let background = app.theme.selection.bg.expect("a selection background");
    terminal.backend().buffer()[(x, y)].bg == background
}

#[test]
fn the_selected_row_is_highlighted_in_the_focused_pane_only() {
    let mut app = seeded();
    let mut terminal = Terminal::new(TestBackend::new(110, 16)).expect("terminal");
    terminal
        .draw(|frame| super::draw(frame, &app))
        .expect("draw");
    // Under the title bar, Home is the sidebar's seventh row; "Pay rent"
    // the list's first.
    assert!(!selected(&terminal, &app, 2, 8));
    assert!(selected(&terminal, &app, 26, 2));
    app.focus = Pane::Sidebar;
    terminal
        .draw(|frame| super::draw(frame, &app))
        .expect("draw");
    assert!(selected(&terminal, &app, 2, 8));
    assert!(!selected(&terminal, &app, 26, 2));
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
fn completed_is_grouped_by_day() {
    let mut app = seeded();
    let on = |day: &str| {
        json!({ "status": "completed",
                "completedDateTime": { "dateTime": format!("{day}T00:00:00.0000000"), "timeZone": "UTC" } })
    };
    let tasks = vec![
        task(
            "c0",
            "Just now",
            json!({ "status": "completed", "sync_state": "pending" }),
        ),
        task("c1", "Ship blueprint", on("2026-09-24")),
        task("c2", "Call Sam", on("2026-09-23")),
        task("c3", "Pay rent", on("2026-09-21")),
    ];
    let effects = app.update(Msg::Event(ms_todo_protocol::Event::ResyncNeeded));
    app.update(Msg::Response {
        tag: effects[0].tag,
        result: Ok(ms_todo_protocol::ResponseData::Seed(seed(
            Scope::Completed,
            tasks,
        ))),
    });
    app.task_index = 2;
    insta::assert_snapshot!(render(&app));
}

#[test]
fn one_due_date_for_several_tasks_shows_the_day_it_reads() {
    let mut app = seeded();
    app.mode = Mode::SettingDue {
        ids: vec!["t1".into(), "t2".into()],
        what: "2 tasks".into(),
        input: crate::app::line_editor::LineEditor::single("next mon"),
        error: None,
    };
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
    app.mode = Mode::Filtering {
        input: crate::app::line_editor::LineEditor::single("milk"),
    };
    let filtering = render(&app);
    app.mode = Mode::ConfirmDelete {
        ids: vec!["t2".into()],
        what: "\"Call Sam\"".into(),
    };
    let confirming = render(&app);
    let last = |frame: &str| frame.lines().last().unwrap_or_default().to_owned();
    insta::assert_snapshot!(format!("{}\n{}", last(&filtering), last(&confirming)));
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

/// Important, recurring, a reminder and due, next to tasks with only
/// some of them: each marker keeps its own cells, in both glyph sets.
#[test]
fn task_list_markers_never_overlap() {
    let due = json!({ "dateTime": "2026-10-01T00:00:00.0000000", "timeZone": "Europe/London" });
    let recurrence =
        json!({ "pattern": { "type": "absoluteMonthly", "interval": 1, "dayOfMonth": 1 } });
    let reminder =
        json!({ "dateTime": "2026-09-30T09:00:00.0000000", "timeZone": "Europe/London" });
    let tasks = vec![
        task(
            "t1",
            "Pay rent",
            json!({ "importance": "high", "dueDateTime": due, "recurrence": recurrence,
                    "isReminderOn": true, "reminderDateTime": reminder }),
        ),
        task(
            "t2",
            "Update the budget",
            json!({ "importance": "high", "dueDateTime": due, "recurrence": recurrence }),
        ),
        task(
            "t3",
            "Water plants",
            json!({ "dueDateTime": due, "recurrence": recurrence }),
        ),
        task(
            "t4",
            "Call Sam",
            json!({ "isReminderOn": true, "reminderDateTime": reminder }),
        ),
    ];
    let mut app = seeded();
    let effects = app.update(Msg::Connected);
    answer_seed(
        &mut app,
        &effects[0],
        seed(Scope::List { id: "home".into() }, tasks.clone()),
    );
    let unicode = render(&app);
    app.glyphs = ASCII;
    insta::assert_snapshot!(format!("{unicode}\n{}", render(&app)));
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
    app.update(Msg::Action(Action::EditHere));
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

/// `e` puts the field picker in the hint bar; `i` there, the levels.
#[test]
fn the_field_picker_and_the_importance_levels() {
    use crate::action::Action;
    use crate::app::edit::Field;
    let mut app = seeded();
    app.update(Msg::Action(Action::Edit));
    let picker = render(&app);
    app.update(Msg::Action(Action::EditField(Field::Importance)));
    let levels = render(&app);
    let last = |frame: &str| frame.lines().last().unwrap_or_default().to_owned();
    insta::assert_snapshot!(format!("{}\n{levels}", last(&picker)));
}

/// The cursor mid-title reverses the character under it; at the end, the
/// cursor glyph follows the text. The hint bar says how to move.
#[test]
fn an_edit_shows_its_cursor() {
    use crate::action::Action;
    use crate::app::edit::Field;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = seeded();
    app.update(Msg::Action(Action::Edit));
    app.update(Msg::Action(Action::EditField(Field::Title)));
    let at_end = render(&app);
    app.update(Msg::Key(KeyEvent::new(
        KeyCode::Char('b'),
        KeyModifiers::ALT,
    )));
    let mut terminal = Terminal::new(TestBackend::new(110, 16)).expect("terminal");
    terminal
        .draw(|frame| super::draw(frame, &app))
        .expect("draw");
    // "Title      Pay rent" in the detail pane: the cursor is on the `r`.
    let buffer = terminal.backend().buffer();
    let reversed: Vec<(u16, &str)> = (77..109)
        .filter(|&x| buffer[(x, 2)].modifier.contains(Modifier::REVERSED))
        .map(|x| (x, buffer[(x, 2)].symbol()))
        .collect();
    assert_eq!(reversed, [(92, "r")]);
    insta::assert_snapshot!(at_end);
}

#[test]
fn the_notes_editor_takes_several_lines() {
    use crate::action::Action;
    use crate::app::edit::Field;
    let mut app = seeded();
    app.update(Msg::Action(Action::Edit));
    app.update(Msg::Action(Action::EditField(Field::Notes)));
    for ch in "Standing order".chars() {
        app.update(Msg::Char(ch));
    }
    app.update(Msg::Action(Action::Newline));
    for ch in "on the 1st".chars() {
        app.update(Msg::Char(ch));
    }
    let mut terminal = Terminal::new(TestBackend::new(110, 20)).expect("terminal");
    terminal
        .draw(|frame| super::draw(frame, &app))
        .expect("draw");
    insta::assert_snapshot!(terminal.backend().to_string());
}

/// What a typed date resolves to shows under it as it's typed.
#[test]
fn the_resolved_date_shows_under_the_input() {
    use crate::action::Action;
    use crate::app::edit::Field;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = seeded();
    app.update(Msg::Action(Action::Edit));
    app.update(Msg::Action(Action::EditField(Field::Due)));
    app.update(Msg::Key(KeyEvent::new(
        KeyCode::Char('u'),
        KeyModifiers::CONTROL,
    )));
    let mut frames = Vec::new();
    for typed in ["next fri", "yesterday", "tomo"] {
        app.update(Msg::Key(KeyEvent::new(
            KeyCode::Char('u'),
            KeyModifiers::CONTROL,
        )));
        for ch in typed.chars() {
            app.update(Msg::Char(ch));
        }
        // The detail pane's Due rows.
        let frame = render(&app);
        frames.push(
            frame
                .lines()
                .skip(6)
                .take(3)
                .map(|line| line.chars().skip(76).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
    insta::assert_snapshot!(frames.join("\n\n"));
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
/// highlighted row is always the field under the cursor.
#[test]
fn the_detail_cursor_follows_the_rows_on_screen() {
    use crate::app::edit::Field;
    let mut app = seeded();
    app.focus = Pane::Detail;
    let mut terminal = Terminal::new(TestBackend::new(110, 20)).expect("terminal");
    let mut rows = Vec::new();
    for field in Field::ALL {
        app.detail_row = crate::app::steps::DetailRow::Field(field);
        terminal
            .draw(|frame| super::draw(frame, &app))
            .expect("draw");
        // The detail pane's first column inside its border.
        let x = 77;
        let row = (0..20)
            .find(|&y| selected(&terminal, &app, x, y))
            .expect("a highlighted row");
        let buffer = terminal.backend().buffer();
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
    app.lists[1].name = evil.into();
    // A folder's name comes from another device's extension.
    app.lists[0].folder = Some(evil.into());
    app.show(Level::Error, evil);
    let mut screens = vec![render(&app)];
    // Editing the title shows it as it came, in the line editor.
    app.update(Msg::Action(crate::action::Action::EditField(
        crate::app::edit::Field::Title,
    )));
    screens.push(render(&app));
    app.update(Msg::Action(crate::action::Action::Cancel));
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

#[test]
fn the_sidebar_with_folders_one_collapsed() {
    let mut app = crate::app::folders::tests::foldered();
    app.collapsed.insert("Projects".into());
    app.focus = Pane::Sidebar;
    let mut shown = render(&app);
    app.mode = Mode::MovingList {
        list_id: "home".into(),
        input: crate::app::line_editor::LineEditor::single("Ar"),
    };
    shown.push_str(&render(&app));
    insta::assert_snapshot!(shown);
}

#[test]
fn the_sidebar_with_folders_in_ascii() {
    let mut app = crate::app::folders::tests::foldered();
    app.glyphs = ASCII;
    app.collapsed.insert("Projects".into());
    insta::assert_snapshot!(render(&app));
}

/// The main screen with its colours, one snapshot per theme: the
/// terminal's own palette, an RGB one, and `NO_COLOR`.
fn render_styled(app: &App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(110, 16)).expect("terminal");
    terminal
        .draw(|frame| super::draw(frame, app))
        .expect("draw");
    format!("{:?}", terminal.backend().buffer())
}

fn themed(name: &'static str, capability: crate::theme::Capability) -> App {
    let mut app = seeded();
    app.task_index = 0;
    app.with_theme(crate::theme::ThemeChoice {
        builtin: crate::theme::builtin(name).expect("a built-in theme"),
        capability,
        ..Default::default()
    })
}

#[test]
fn the_terminal_theme() {
    let app = themed("terminal", crate::theme::Capability::Ansi256);
    insta::assert_snapshot!(render_styled(&app));
}

#[test]
fn catppuccin_mocha_in_truecolor() {
    let app = themed("catppuccin-mocha", crate::theme::Capability::Truecolor);
    insta::assert_snapshot!(render_styled(&app));
}

#[test]
fn kanagawa_in_truecolor() {
    let app = themed("kanagawa", crate::theme::Capability::Truecolor);
    insta::assert_snapshot!(render_styled(&app));
}

#[test]
fn no_color_is_monochrome() {
    let app = themed("catppuccin-mocha", crate::theme::Capability::Monochrome);
    let screen = render_styled(&app);
    // Only the terminal's own colours: bold, dim and reverse carry it.
    assert!(!screen.contains("Rgb") && !screen.contains("Indexed"));
    for colour in ["Red", "Cyan", "Yellow", "DarkGray"] {
        assert!(!screen.contains(colour), "{colour} with NO_COLOR");
    }
    insta::assert_snapshot!(screen);
}

#[test]
fn the_theme_picker_lists_every_theme() {
    let mut app = seeded();
    app.task_index = 0;
    app.mode = Mode::Themes {
        index: 1,
        before: &crate::theme::BUILTIN[0],
    };
    insta::assert_snapshot!(render(&app));
}

#[test]
fn the_link_picker_lists_each_link_and_marks_one_that_wont_open() {
    let mut app = seeded();
    app.task_index = 0;
    app.mode = Mode::Links {
        links: ms_todo_core::links::links(
            [
                ("https://mail.example.com/1", Some("The email")),
                ("javascript:alert(1)", None),
            ],
            Some(("and [docs](https://docs.example.com)", false)),
        ),
        index: 1,
    };
    insta::assert_snapshot!(render(&app));
}

#[test]
fn urls_in_notes_are_drawn_as_links() {
    let mut app = seeded();
    app.task_index = 0;
    app.tasks[0].body = Some(crate::app::task::Body {
        content: "see https://example.com/a now".into(),
        html: false,
    });
    let mut terminal = Terminal::new(TestBackend::new(110, 24)).expect("terminal");
    terminal
        .draw(|frame| super::draw(frame, &app))
        .expect("draw");
    let buffer = terminal.backend().buffer();
    let row = (0..24)
        .find(|&y| {
            (0..110)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .contains("see https://example.com/a now")
        })
        .expect("the notes line");
    let line: String = (0..110).map(|x| buffer[(x, row)].symbol()).collect();
    // A column, not a byte: the borders are several bytes each.
    let byte = line.find("https").expect("url");
    let start = u16::try_from(line[..byte].chars().count()).expect("fits");
    let link = app.theme.link;
    assert_eq!(buffer[(start, row)].modifier, link.add_modifier);
    assert_ne!(
        buffer[(start - 2, row)].modifier,
        link.add_modifier,
        "\"see\" isn't a link"
    );
}

/// Quick add's modal with `text` typed, read as it's typed.
fn adding(mut app: App, text: &str) -> App {
    app.update(Msg::Action(crate::action::Action::Add));
    for ch in text.chars() {
        app.update(Msg::Char(ch));
    }
    app
}

const QUICK_ADD: &str = "Pay rent every 1st #Tasks p1 9am @bills +myday";

#[test]
fn the_add_modal_highlights_what_it_read_and_previews_the_task() {
    let app = adding(seeded(), QUICK_ADD);
    let mut terminal = Terminal::new(TestBackend::new(110, 16)).expect("terminal");
    terminal
        .draw(|frame| super::draw(frame, &app))
        .expect("draw");
    let screen = terminal.backend().to_string();
    // The status line and the hint bar are still there, under the modal.
    assert!(screen.lines().last().unwrap_or_default().contains("Enter"));
    insta::assert_snapshot!(screen);
    // Each part in its kind's style: find the row with the text on it.
    let buffer = terminal.backend().buffer();
    let row = (0..16)
        .find(|y| {
            (0..110)
                .map(|x| buffer[(x, *y)].symbol())
                .collect::<String>()
                .contains("> Pay rent")
        })
        .expect("the text's row");
    let line: String = (0..110).map(|x| buffer[(x, row)].symbol()).collect();
    let style_of = |needle: &str| {
        let at = line.find(needle).expect(needle);
        let x = u16::try_from(line[..at].chars().count()).expect("x");
        buffer[(x, row)].fg
    };
    let theme = &app.theme;
    assert_eq!(Some(style_of("every 1st")), theme.nlp_recurrence.fg);
    assert_eq!(Some(style_of("#Tasks")), theme.nlp_list.fg);
    assert_eq!(Some(style_of("p1")), theme.nlp_priority.fg);
    assert_eq!(Some(style_of("9am")), theme.nlp_date.fg);
    assert_eq!(Some(style_of("@bills")), theme.nlp_label.fg);
}

#[test]
fn the_add_modal_in_catppuccin_mocha() {
    let app = adding(
        themed("catppuccin-mocha", crate::theme::Capability::Truecolor),
        QUICK_ADD,
    );
    insta::assert_snapshot!(render_styled(&app));
}

#[test]
fn the_add_modal_in_the_terminal_theme() {
    let app = adding(
        themed("terminal", crate::theme::Capability::Ansi256),
        QUICK_ADD,
    );
    insta::assert_snapshot!(render_styled(&app));
}

#[test]
fn the_add_modal_on_a_narrow_terminal() {
    let app = adding(seeded(), QUICK_ADD);
    let mut terminal = Terminal::new(TestBackend::new(48, 20)).expect("terminal");
    terminal
        .draw(|frame| super::draw(frame, &app))
        .expect("draw");
    insta::assert_snapshot!(terminal.backend().to_string());
}

#[test]
fn the_add_modal_taking_the_text_literally() {
    let mut app = adding(seeded(), "Email Friday report tomorrow");
    app.update(Msg::Action(crate::action::Action::ToggleParse));
    insta::assert_snapshot!(render(&app));
}

#[test]
fn the_add_modal_offers_a_likely_list_for_the_inbox() {
    let mut app = App::new(crate::glyphs::UNICODE, crate::app::tests::clock());
    app.version = "9.9.9";
    let effects = app.update(Msg::Connected);
    crate::app::tests::answer_seed(
        &mut app,
        &effects[0],
        crate::app::tests::seed(
            ms_todo_protocol::Scope::List { id: "tasks".into() },
            Vec::new(),
        ),
    );
    let mut app = adding(app, "mow the lawn");
    for _ in 0..crate::app::list_hint::QUIET_TICKS {
        app.update(Msg::Tick(crate::app::tests::clock()));
    }
    app.update(Msg::Response {
        tag: crate::app::Tag::ListHint,
        result: Ok(ms_todo_protocol::ResponseData::ListSuggestion {
            suggestion: Some(ms_todo_protocol::ListSuggestion {
                list_id: "home".into(),
                list_name: "Home".into(),
                confidence: 0.9,
            }),
        }),
    });
    let screen = render(&app);
    assert!(
        screen.contains("\u{2192} Home? (Ctrl-l to accept)"),
        "{screen}"
    );
    insta::assert_snapshot!(screen);
}

#[test]
fn my_day_with_its_suggestions() {
    let mut app = crate::app::my_day::tests::in_my_day();
    app.task_index = 2;
    insta::assert_snapshot!(render(&app));
}

/// A task with steps (one checked) and a link: its row counts them, and
/// the detail pane shows each with its checkbox, the cursor on one, then
/// a new step being typed.
#[test]
fn steps_and_the_link_in_the_row_and_the_detail_pane() {
    use crate::action::Action;
    use crate::app::steps::DetailRow;
    let mut app = seeded();
    let effects = app.update(Msg::Event(ms_todo_protocol::Event::ResyncNeeded));
    let paint = task(
        "t9",
        "Paint the room",
        json!({
            "checklistItems": [
                { "id": "c1", "displayName": "Buy paint", "isChecked": true },
                { "id": "c2", "displayName": "Tape the edges", "isChecked": false },
                { "id": "c3", "displayName": "Two coats", "isChecked": false }
            ],
            "linkedResources": [{
                "id": "r1", "webUrl": "https://example.com/colours",
                "applicationName": "ms-todo", "displayName": "Colour chart"
            }]
        }),
    );
    answer_seed(
        &mut app,
        &effects[0],
        seed(Scope::List { id: "home".into() }, vec![paint]),
    );
    app.focus = Pane::Detail;
    app.detail_row = DetailRow::Step(1);
    let mut terminal = Terminal::new(TestBackend::new(110, 20)).expect("terminal");
    terminal
        .draw(|frame| super::draw(frame, &app))
        .expect("draw");
    let browsing = terminal.backend().to_string();
    app.update(Msg::Action(Action::Add));
    for ch in "Clean up".chars() {
        app.update(Msg::Char(ch));
    }
    terminal
        .draw(|frame| super::draw(frame, &app))
        .expect("draw");
    insta::assert_snapshot!(format!("{browsing}\n{}", terminal.backend()));
}

/// A task with files: its row's marker, and the detail pane listing each
/// with its size, one still uploading, the cursor on one; then a path
/// being typed to attach another.
#[test]
fn attachments_in_the_row_and_the_detail_pane() {
    use crate::action::Action;
    use crate::app::steps::DetailRow;
    let mut app = seeded();
    let effects = app.update(Msg::Event(ms_todo_protocol::Event::ResyncNeeded));
    let taxes = task(
        "t9",
        "File taxes",
        json!({
            "hasAttachments": true,
            "attachments": [
                { "id": "A1", "name": "return.pdf", "size": 48_000 },
                { "id": "A2", "name": "scan.tiff", "size": 9_437_184 },
                { "id": "local-1", "name": "receipt.jpg", "size": 900 }
            ]
        }),
    );
    answer_seed(
        &mut app,
        &effects[0],
        seed(Scope::List { id: "home".into() }, vec![taxes]),
    );
    app.focus = Pane::Detail;
    app.detail_row = DetailRow::Attachment(1);
    let mut terminal = Terminal::new(TestBackend::new(110, 20)).expect("terminal");
    terminal
        .draw(|frame| super::draw(frame, &app))
        .expect("draw");
    let browsing = terminal.backend().to_string();
    app.update(Msg::Action(Action::Attach));
    for ch in "~/invoice.pdf".chars() {
        app.update(Msg::Char(ch));
    }
    terminal
        .draw(|frame| super::draw(frame, &app))
        .expect("draw");
    insta::assert_snapshot!(format!("{browsing}\n{}", terminal.backend()));
}
