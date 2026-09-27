use ms_todo_protocol::{ErrorPayload, Request, ResponseData, Scope, TaskChange};
use serde_json::json;

use super::*;
use crate::action::Action;
use crate::app::tests::{act, seed, seeded, task};
use crate::app::{Msg, Tag};
use crate::keybindings::Context;

fn suggestion(list_id: &str, name: &str) -> ResponseData {
    ResponseData::ListSuggestion {
        suggestion: Some(ListSuggestion {
            list_id: list_id.into(),
            list_name: name.into(),
            confidence: 0.82,
        }),
    }
}

fn answer(app: &mut App, tag: Tag, result: Result<ResponseData, ErrorPayload>) -> Vec<Effect> {
    app.update(Msg::Response { tag, result })
}

fn asked_about(effects: &[Effect]) -> &str {
    match effects {
        [
            Effect {
                tag: Tag::TriageSuggest,
                request: Request::SuggestList { title },
            },
        ] => title,
        other => unreachable!("not one question: {other:?}"),
    }
}

/// Triage started from the palette on an inbox of three open tasks and a
/// completed one; the first asked about.
pub(crate) fn triaging() -> App {
    let mut app = seeded();
    act(&mut app, Action::Palette);
    for ch in "suggest lists".chars() {
        app.update(Msg::Char(ch));
    }
    let effects = act(&mut app, Action::Submit);
    assert_eq!(
        effects,
        [Effect {
            tag: Tag::TriageInbox,
            request: Request::Seed {
                scope: Some(Scope::List { id: "tasks".into() }),
                search: None,
                include_deferred: false,
                semantic: false,
            },
        }]
    );
    assert_eq!(app.context(), Context::Triage);
    let inbox = vec![
        task("i1", "Buy paint", json!({ "list_id": "tasks" })),
        task("i2", "Fix the gate", json!({ "list_id": "tasks" })),
        task(
            "i3",
            "Old",
            json!({ "list_id": "tasks", "status": "completed" }),
        ),
        task("i4", "Call the bank", json!({ "list_id": "tasks" })),
    ];
    let seed = seed(Scope::List { id: "tasks".into() }, inbox);
    let effects = answer(
        &mut app,
        Tag::TriageInbox,
        Ok(ResponseData::Seed(Box::new(seed))),
    );
    assert_eq!(asked_about(&effects), "Buy paint");
    app
}

fn triage(app: &App) -> &Triage {
    match &app.mode {
        Mode::Triage(triage) => triage,
        other => unreachable!("not triaging: {other:?}"),
    }
}

#[test]
fn each_open_inbox_task_is_suggested_a_list_and_moved_with_one_key() {
    let mut app = triaging();
    assert_eq!(triage(&app).tasks.len(), 3, "the completed one is left out");
    assert_eq!(triage(&app).suggestion, Suggestion::Asking);
    // Nothing to move until a list is suggested.
    assert!(act(&mut app, Action::Submit).is_empty());
    answer(&mut app, Tag::TriageSuggest, Ok(suggestion("home", "Home")));
    assert!(
        matches!(&triage(&app).suggestion, Suggestion::Found(list) if list.list_name == "Home")
    );
    let effects = act(&mut app, Action::Submit);
    assert_eq!(effects.len(), 2, "the move, then the next question");
    assert_eq!(
        effects[0].request,
        Request::ChangeTasks {
            tasks: vec!["i1".into()],
            list: None,
            select: None,
            change: TaskChange::Move { to: "home".into() },
            dry_run: false,
            op_id: None,
            idempotency_key: None,
        }
    );
    assert_eq!(effects[0].tag, Tag::Write(Write::Move));
    assert_eq!(asked_about(&effects[1..]), "Fix the gate");
    // No likely list: nothing to move; s leaves it.
    answer(
        &mut app,
        Tag::TriageSuggest,
        Ok(ResponseData::ListSuggestion { suggestion: None }),
    );
    assert_eq!(triage(&app).suggestion, Suggestion::Nothing);
    assert!(act(&mut app, Action::Submit).is_empty());
    assert_eq!(asked_about(&act(&mut app, Action::Skip)), "Call the bank");
    // The inbox itself is no suggestion.
    answer(
        &mut app,
        Tag::TriageSuggest,
        Ok(suggestion("tasks", "Tasks")),
    );
    assert_eq!(triage(&app).suggestion, Suggestion::Nothing);
    assert!(act(&mut app, Action::Skip).is_empty());
    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(
        app.banner.as_ref().map(|banner| banner.text.as_str()),
        Some("Inbox done: moved 1, left 2 in the inbox")
    );
}

#[test]
fn an_answer_for_a_skipped_task_is_dropped_and_the_next_asked_about() {
    let mut app = triaging();
    // Skipped before its answer came: no second question in flight.
    assert!(act(&mut app, Action::Skip).is_empty());
    assert_eq!(triage(&app).at, 1);
    let effects = answer(&mut app, Tag::TriageSuggest, Ok(suggestion("home", "Home")));
    assert_eq!(asked_about(&effects), "Fix the gate");
    assert_eq!(triage(&app).suggestion, Suggestion::Asking);
}

#[test]
fn suggestions_off_says_how_to_turn_them_on_and_esc_stops() {
    let mut app = triaging();
    let off = "list suggestions are off; to turn them on, set `enabled = true` under [suggest]";
    answer(
        &mut app,
        Tag::TriageSuggest,
        Err(ErrorPayload {
            kind: "invalid_input".into(),
            message: off.into(),
            ..ErrorPayload::default()
        }),
    );
    assert_eq!(triage(&app).suggestion, Suggestion::Failed(off.into()));
    assert!(act(&mut app, Action::Cancel).is_empty());
    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(
        app.banner.as_ref().map(|banner| banner.text.as_str()),
        Some("Stopped")
    );
}

#[test]
fn an_empty_inbox_ends_at_once() {
    let mut app = seeded();
    let effects = act(&mut app, Action::TriageInbox);
    let empty = seed(Scope::List { id: "tasks".into() }, Vec::new());
    assert!(
        answer(
            &mut app,
            effects[0].tag,
            Ok(ResponseData::Seed(Box::new(empty)))
        )
        .is_empty()
    );
    assert_eq!(app.mode, Mode::Normal);
    assert!(
        app.banner
            .as_ref()
            .is_some_and(|banner| banner.text.contains("no open tasks"))
    );
}
