//! Quick add's list suggestion through `update`: asked once typing
//! pauses, only for a task headed for the inbox, one request at a time,
//! and `Ctrl-l` types the list in.

use ms_todo_protocol::{ErrorPayload, ListSuggestion, Request, ResponseData, Scope};

use super::QUIET_TICKS;
use crate::action::Action;
use crate::app::tests::{act, answer_seed, clock, seed, seeded};
use crate::app::{App, Effect, Mode, Msg, Tag};
use crate::glyphs::UNICODE;

/// Showing the inbox, "Tasks".
fn in_the_inbox() -> App {
    let mut app = App::new(UNICODE, clock());
    let effects = app.update(Msg::Connected);
    answer_seed(
        &mut app,
        &effects[0],
        seed(Scope::List { id: "tasks".into() }, Vec::new()),
    );
    app
}

fn typed(app: &mut App, text: &str) -> Vec<Effect> {
    text.chars()
        .flat_map(|ch| app.update(Msg::Char(ch)))
        .collect()
}

fn tick(app: &mut App) -> Vec<Effect> {
    app.update(Msg::Tick(clock()))
}

/// The titles asked about over `ticks` ticks.
fn asked(app: &mut App, ticks: u32) -> Vec<String> {
    (0..ticks)
        .flat_map(|_| tick(app))
        .filter_map(|effect| match effect {
            Effect {
                tag: Tag::ListHint,
                request: Request::SuggestList { title },
            } => Some(title),
            _ => None,
        })
        .collect()
}

fn suggested(app: &mut App, name: &str, id: &str) {
    app.update(Msg::Response {
        tag: Tag::ListHint,
        result: Ok(ResponseData::ListSuggestion {
            suggestion: Some(ListSuggestion {
                list_id: id.into(),
                list_name: name.into(),
                confidence: 0.9,
            }),
        }),
    });
}

fn text(app: &App) -> String {
    match &app.mode {
        Mode::Adding { input, .. } => input.text(),
        other => unreachable!("{other:?}"),
    }
}

#[test]
fn asked_once_typing_pauses_never_per_key() {
    let mut app = in_the_inbox();
    act(&mut app, Action::Add);
    let effects = typed(&mut app, "pay council tax tomorrow");
    assert!(effects.is_empty(), "a key never asks: {effects:?}");
    assert!(asked(&mut app, QUIET_TICKS - 1).is_empty());
    // The title as quick add reads it, without the date.
    assert_eq!(asked(&mut app, 1), ["pay council tax"]);
    // Asked once, however long the pause.
    assert!(asked(&mut app, 8).is_empty());
}

#[test]
fn a_likely_list_shows_and_ctrl_l_types_it_in() {
    let mut app = in_the_inbox();
    act(&mut app, Action::Add);
    typed(&mut app, "mow the lawn");
    assert_eq!(asked(&mut app, QUIET_TICKS), ["mow the lawn"]);
    assert_eq!(app.list_hint(), None, "nothing until it answers");
    suggested(&mut app, "Home", "home");
    assert_eq!(
        app.list_hint().map(|list| list.list_id.as_str()),
        Some("home")
    );

    act(&mut app, Action::AcceptList);
    assert_eq!(text(&app), "mow the lawn #Home");
    assert_eq!(app.list_hint(), None, "it has a list now");
    let effects = act(&mut app, Action::Submit);
    match effects.as_slice() {
        [
            Effect {
                request: Request::AddTask { task, .. },
                ..
            },
        ] => {
            assert_eq!(task.title, "mow the lawn");
            assert_eq!(task.list.as_deref(), Some("home"));
        }
        other => unreachable!("{other:?}"),
    }
}

#[test]
fn a_hint_is_for_the_title_it_answered() {
    let mut app = in_the_inbox();
    act(&mut app, Action::Add);
    typed(&mut app, "mow the lawn");
    asked(&mut app, QUIET_TICKS);
    suggested(&mut app, "Home", "home");
    typed(&mut app, " and edges");
    assert_eq!(app.list_hint(), None, "stale for the new text");
    // Nothing to accept.
    act(&mut app, Action::AcceptList);
    assert_eq!(text(&app), "mow the lawn and edges");
}

#[test]
fn one_request_at_a_time_then_the_text_as_it_is_now() {
    let mut app = in_the_inbox();
    act(&mut app, Action::Add);
    typed(&mut app, "pay rent");
    assert_eq!(asked(&mut app, QUIET_TICKS), ["pay rent"]);
    typed(&mut app, " and bills");
    assert!(asked(&mut app, 5).is_empty(), "the first is still out");
    app.update(Msg::Response {
        tag: Tag::ListHint,
        result: Ok(ResponseData::ListSuggestion { suggestion: None }),
    });
    assert_eq!(app.list_hint(), None);
    assert_eq!(asked(&mut app, 1), ["pay rent and bills"]);
}

#[test]
fn a_task_with_a_list_is_never_asked_about() {
    // Added from a list other than the inbox.
    let mut app = seeded();
    act(&mut app, Action::Add);
    typed(&mut app, "mow the lawn");
    assert!(asked(&mut app, 4).is_empty());
    // A `#List` typed.
    let mut app = in_the_inbox();
    act(&mut app, Action::Add);
    typed(&mut app, "mow the lawn #Home");
    assert!(asked(&mut app, 4).is_empty());
    // Too short to say anything.
    act(&mut app, Action::Cancel);
    act(&mut app, Action::Add);
    typed(&mut app, "go");
    assert!(asked(&mut app, 4).is_empty());
}

#[test]
fn off_is_never_asked_again_this_session_but_a_lost_answer_is() {
    let mut app = in_the_inbox();
    act(&mut app, Action::Add);
    typed(&mut app, "pay rent");
    asked(&mut app, QUIET_TICKS);
    app.update(Msg::Response {
        tag: Tag::ListHint,
        result: Err(ErrorPayload {
            kind: "daemon_unavailable".into(),
            message: "the daemon isn't running".into(),
            ..ErrorPayload::default()
        }),
    });
    assert_eq!(asked(&mut app, 1), ["pay rent"], "asked again");
    app.update(Msg::Response {
        tag: Tag::ListHint,
        result: Err(ErrorPayload {
            kind: "invalid_input".into(),
            message: "list suggestions are off".into(),
            ..ErrorPayload::default()
        }),
    });
    assert!(app.banner.is_none(), "off is nothing to report");
    typed(&mut app, " now");
    assert!(asked(&mut app, 8).is_empty());
}
