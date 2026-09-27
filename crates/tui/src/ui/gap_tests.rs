//! Frames for what D-066 added: the Start and Repeat fields, clearing
//! several due dates, reordering the sidebar, the link's other fields,
//! the palette's recent commands and the inbox triage.

use ms_todo_protocol::{ErrorPayload, ListSuggestion, ResponseData};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::action::Action;
use crate::app::edit::Field;
use crate::app::folders::tests::foldered;
use crate::app::steps::DetailRow;
use crate::app::tests::{act, seeded};
use crate::app::triage::tests::triaging;
use crate::app::{App, Msg, Pane, Tag};

fn render(app: &App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(110, 24)).expect("terminal");
    terminal
        .draw(|frame| super::draw(frame, app))
        .expect("draw");
    terminal.backend().to_string()
}

fn typed(app: &mut App, text: &str) {
    for ch in text.chars() {
        app.update(Msg::Char(ch));
    }
}

/// Start on a task with no start date, read as it's typed; Repeat, read
/// with its first due date; a start date on a repeating task, refused.
#[test]
fn the_start_and_repeat_fields() {
    let mut app = seeded();
    act(&mut app, Action::MoveDown);
    act(&mut app, Action::Edit);
    act(&mut app, Action::EditField(Field::Start));
    typed(&mut app, "next mon");
    let start = render(&app);
    act(&mut app, Action::Cancel);
    app.focus = Pane::Tasks;
    act(&mut app, Action::Edit);
    act(&mut app, Action::EditField(Field::Repeat));
    typed(&mut app, "every 2 weeks on tue");
    let repeat = render(&app);
    act(&mut app, Action::Cancel);
    app.focus = Pane::Tasks;
    act(&mut app, Action::MoveUp);
    act(&mut app, Action::Edit);
    act(&mut app, Action::EditField(Field::Start));
    typed(&mut app, "fri");
    assert!(act(&mut app, Action::Submit).is_empty());
    insta::assert_snapshot!(format!("{start}\n{repeat}\n{}", render(&app)));
}

/// Nothing typed for several tasks' due date says Enter clears them, and
/// Enter asks first.
#[test]
fn clearing_several_due_dates_asks_first() {
    let mut app = seeded();
    act(&mut app, Action::SelectAll);
    act(&mut app, Action::SetDue);
    let last = |frame: &str| frame.lines().last().unwrap_or_default().to_owned();
    let prompt = last(&render(&app));
    act(&mut app, Action::Submit);
    insta::assert_snapshot!(format!("{prompt}\n{}", last(&render(&app))));
}

/// A folder moved up the sidebar with its lists, the cursor still on it.
#[test]
fn a_folder_moved_up_the_sidebar() {
    let mut app = foldered();
    app.focus = Pane::Sidebar;
    app.sidebar_index = app
        .entries()
        .iter()
        .position(|entry| entry.scope().is_none() && format!("{entry:?}").contains("Projects"))
        .expect("Projects");
    assert_eq!(act(&mut app, Action::ReorderUp).len(), 1);
    insta::assert_snapshot!(render(&app));
}

/// The link's editor: Tab from its URL to its name keeps the URL, and
/// shows every field.
#[test]
fn the_link_editor_tabs_to_its_name() {
    let mut app = seeded();
    app.focus = Pane::Detail;
    app.detail_row = DetailRow::Link;
    act(&mut app, Action::EditHere);
    typed(&mut app, "https://example.com/rent");
    act(&mut app, Action::Complete);
    typed(&mut app, "Landlord portal");
    insta::assert_snapshot!(render(&app));
}

/// The palette opens with the last commands run from it first.
#[test]
fn the_palette_puts_recent_commands_first() {
    let mut app = seeded();
    for query in ["sync", "go to planned", "diag"] {
        act(&mut app, Action::Palette);
        typed(&mut app, query);
        act(&mut app, Action::Submit);
        app.mode = crate::app::Mode::Normal;
    }
    app.update(Msg::Response {
        tag: Tag::Sync,
        result: Ok(ResponseData::Ack),
    });
    app.banner = None;
    act(&mut app, Action::Palette);
    insta::assert_snapshot!(render(&app));
}

/// Suggest lists for inbox: asking, a list found, none likely, and
/// suggestions off.
#[test]
fn the_inbox_triage() {
    let mut app = triaging();
    let asking = render(&app);
    app.update(Msg::Response {
        tag: Tag::TriageSuggest,
        result: Ok(ResponseData::ListSuggestion {
            suggestion: Some(ListSuggestion {
                list_id: "home".into(),
                list_name: "Home".into(),
                confidence: 0.82,
            }),
        }),
    });
    let found = render(&app);
    act(&mut app, Action::Skip);
    app.update(Msg::Response {
        tag: Tag::TriageSuggest,
        result: Ok(ResponseData::ListSuggestion { suggestion: None }),
    });
    let nothing = render(&app);
    act(&mut app, Action::Skip);
    app.update(Msg::Response {
        tag: Tag::TriageSuggest,
        result: Err(ErrorPayload {
            kind: "invalid_input".into(),
            message: "list suggestions are off; to turn them on, set `enabled = true` under \
                      [suggest] in ~/.config/ms-todo/config.toml, then restart the daemon with \
                      `ms-todo daemon stop`"
                .into(),
            ..ErrorPayload::default()
        }),
    });
    insta::assert_snapshot!(format!("{asking}\n{found}\n{nothing}\n{}", render(&app)));
}

#[test]
fn reading_the_inbox() {
    let mut app = seeded();
    act(&mut app, Action::TriageInbox);
    insta::assert_snapshot!(render(&app));
}
