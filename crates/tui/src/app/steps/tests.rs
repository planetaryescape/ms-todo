//! Steps and the link in the detail pane: the cursor, the keys, and the
//! writes they send. No daemon, no terminal.

use ms_todo_protocol::{Request, TaskAction};
use serde_json::json;

use super::*;
use crate::app::tests::{act, answer_seed, applied, scope_home, seed, seeded, task};
use crate::app::{Msg, Tag};
use crate::keybindings::Context;

/// Home's tasks, "Paint" first with two steps (one checked) and a link.
fn painted() -> App {
    let mut app = seeded();
    let effects = app.update(Msg::Event(ms_todo_protocol::Event::ResyncNeeded));
    let paint = task(
        "t9",
        "Paint",
        json!({
            "checklistItems": [
                { "id": "c1", "displayName": "Buy paint", "isChecked": true },
                { "id": "c2", "displayName": "Tape", "isChecked": false }
            ],
            "linkedResources": [{ "id": "r1", "webUrl": "https://example.com/colours", "applicationName": "ms-todo" }]
        }),
    );
    answer_seed(&mut app, &effects[0], seed(scope_home(), vec![paint]));
    app.task_index = 0;
    app.focus = Pane::Detail;
    app
}

/// The request an effect sends, as a task change.
fn sent(effects: &[Effect]) -> (Vec<String>, TaskChange) {
    assert_eq!(effects.len(), 1, "{effects:?}");
    match &effects[0].request {
        Request::ChangeTasks { tasks, change, .. } => (tasks.clone(), change.clone()),
        other => unreachable!("not a change: {other:?}"),
    }
}

fn to_row(app: &mut App, row: DetailRow) {
    act(app, Action::JumpTop);
    for _ in 0..20 {
        if app.detail_row_now() == Some(row) {
            return;
        }
        act(app, Action::MoveDown);
    }
    unreachable!("never reached {row:?}");
}

#[test]
fn the_cursor_runs_through_the_steps_and_the_link_before_the_notes() {
    let mut app = painted();
    act(&mut app, Action::JumpTop);
    let mut seen = vec![app.detail_row_now().expect("row")];
    for _ in 0..9 {
        act(&mut app, Action::MoveDown);
        seen.push(app.detail_row_now().expect("row"));
    }
    seen.dedup();
    assert_eq!(
        seen,
        [
            DetailRow::Field(Field::Title),
            DetailRow::Field(Field::Due),
            DetailRow::Field(Field::Reminder),
            DetailRow::Field(Field::Importance),
            DetailRow::Steps,
            DetailRow::Step(0),
            DetailRow::Step(1),
            DetailRow::Link,
            DetailRow::Field(Field::Notes),
        ]
    );
    to_row(&mut app, DetailRow::Step(1));
    assert_eq!(app.context(), Context::Steps);
    to_row(&mut app, DetailRow::Field(Field::Due));
    assert_eq!(app.context(), Context::Detail);
}

#[test]
fn space_checks_or_unchecks_the_step_under_the_cursor() {
    let mut app = painted();
    to_row(&mut app, DetailRow::Step(0));
    assert_eq!(
        sent(&act(&mut app, Action::ToggleStep)),
        (
            vec!["t9".to_owned()],
            TaskChange::CheckSteps {
                steps: vec!["c1".into()],
                checked: false
            }
        )
    );
    to_row(&mut app, DetailRow::Step(1));
    let (_, change) = sent(&act(&mut app, Action::ToggleStep));
    assert!(matches!(
        change,
        TaskChange::CheckSteps { checked: true, .. }
    ));
    to_row(&mut app, DetailRow::Link);
    assert!(act(&mut app, Action::ToggleStep).is_empty(), "not a step");
}

#[test]
fn a_adds_steps_one_after_another_until_esc() {
    let mut app = painted();
    to_row(&mut app, DetailRow::Steps);
    act(&mut app, Action::Add);
    assert_eq!(app.context(), Context::Prompt);
    for ch in "Sand".chars() {
        app.update(Msg::Char(ch));
    }
    let (_, change) = sent(&act(&mut app, Action::Submit));
    assert_eq!(
        change,
        TaskChange::AddSteps {
            steps: vec!["Sand".into()]
        }
    );
    // Still adding, for the next one; Enter on nothing stops.
    assert!(matches!(
        &app.mode,
        Mode::EditingChild { target: ChildTarget::NewStep, input, .. } if input.text().is_empty()
    ));
    assert!(act(&mut app, Action::Submit).is_empty());
    assert_eq!(app.mode, Mode::Normal);
}

#[test]
fn the_answer_draws_the_new_step_at_once() {
    let mut app = painted();
    to_row(&mut app, DetailRow::Steps);
    act(&mut app, Action::Add);
    app.update(Msg::Char('X'));
    let effects = act(&mut app, Action::Submit);
    let answered = task(
        "t9",
        "Paint",
        json!({
            "sync_state": "pending",
            "checklistItems": [
                { "id": "c1", "displayName": "Buy paint", "isChecked": true },
                { "id": "c2", "displayName": "Tape", "isChecked": false },
                { "id": "local-1", "displayName": "X", "isChecked": false }
            ]
        }),
    );
    app.update(Msg::Response {
        tag: effects[0].tag,
        result: Ok(applied(TaskAction::StepAdd, vec![answered])),
    });
    assert_eq!(effects[0].tag, Tag::Write(Write::Edit));
    let names: Vec<&str> = app.tasks[0]
        .steps
        .iter()
        .map(|step| step.name.as_str())
        .collect();
    assert_eq!(names, ["Buy paint", "Tape", "X"]);
}

#[test]
fn e_renames_a_step_and_an_empty_one_is_refused() {
    let mut app = painted();
    to_row(&mut app, DetailRow::Step(1));
    act(&mut app, Action::Edit);
    assert!(matches!(&app.mode, Mode::EditingChild { input, .. } if input.text() == "Tape"));
    app.update(Msg::Char('s'));
    let (_, change) = sent(&act(&mut app, Action::Submit));
    assert_eq!(
        change,
        TaskChange::EditStep {
            step: "c2".into(),
            text: "Tapes".into()
        }
    );
    act(&mut app, Action::EditHere);
    for _ in 0..4 {
        act(&mut app, Action::Backspace);
    }
    assert!(act(&mut app, Action::Submit).is_empty());
    assert!(matches!(
        &app.mode,
        Mode::EditingChild { error: Some(_), .. }
    ));
}

#[test]
fn d_asks_before_deleting_a_step_or_the_link() {
    let mut app = painted();
    to_row(&mut app, DetailRow::Step(0));
    assert!(act(&mut app, Action::Delete).is_empty());
    assert_eq!(app.context(), Context::Confirm);
    let (_, change) = sent(&act(&mut app, Action::Confirm));
    assert_eq!(
        change,
        TaskChange::DeleteSteps {
            steps: vec!["c1".into()]
        }
    );
    to_row(&mut app, DetailRow::Link);
    act(&mut app, Action::Delete);
    assert!(act(&mut app, Action::Cancel).is_empty(), "n keeps it");
    act(&mut app, Action::Delete);
    let (_, change) = sent(&act(&mut app, Action::Confirm));
    assert_eq!(
        change,
        TaskChange::DeleteLink {
            link: Some("r1".into())
        }
    );
}

#[test]
fn enter_on_the_link_edits_its_url_or_adds_one() {
    let mut app = painted();
    to_row(&mut app, DetailRow::Link);
    act(&mut app, Action::EditHere);
    assert!(
        matches!(&app.mode, Mode::EditingChild { input, .. } if input.text() == "https://example.com/colours")
    );
    for _ in 0.."colours".len() {
        act(&mut app, Action::Backspace);
    }
    for ch in "sizes".chars() {
        app.update(Msg::Char(ch));
    }
    let (_, change) = sent(&act(&mut app, Action::Submit));
    assert_eq!(
        change,
        TaskChange::EditLink(LinkEdit {
            link: Some("r1".into()),
            url: Some("https://example.com/sizes".into()),
            ..LinkEdit::default()
        })
    );

    // A task with no link: Enter adds one, and a bad URL is refused.
    let mut app = seeded();
    app.focus = Pane::Detail;
    to_row(&mut app, DetailRow::Link);
    act(&mut app, Action::EditHere);
    for ch in "nope".chars() {
        app.update(Msg::Char(ch));
    }
    assert!(act(&mut app, Action::Submit).is_empty());
    assert!(matches!(
        &app.mode,
        Mode::EditingChild { error: Some(_), .. }
    ));
    let mut app = seeded();
    app.focus = Pane::Detail;
    to_row(&mut app, DetailRow::Link);
    act(&mut app, Action::EditHere);
    for ch in "https://a.example".chars() {
        app.update(Msg::Char(ch));
    }
    let (_, change) = sent(&act(&mut app, Action::Submit));
    assert!(
        matches!(change, TaskChange::AddLink(NewLink { url, .. }) if url == "https://a.example")
    );
}

#[test]
fn a_task_with_fewer_steps_keeps_the_cursor_on_its_steps() {
    let mut app = painted();
    to_row(&mut app, DetailRow::Step(1));
    // Moving to a task with no steps: the cursor sits on the heading.
    app.focus = Pane::Tasks;
    act(&mut app, Action::MoveDown);
    app.detail_row = DetailRow::Step(1);
    let first = app.tasks[0].clone();
    let mut none = first.clone();
    none.steps.clear();
    assert_eq!(DetailRow::Step(1).on(&none), DetailRow::Steps);
    assert_eq!(DetailRow::Step(5).on(&first), DetailRow::Step(1));
}

/// Home again with "Paint" holding `steps`, as a refresh brings it.
fn refreshed(app: &mut App, steps: serde_json::Value) {
    let effects = app.update(Msg::Event(ms_todo_protocol::Event::ResyncNeeded));
    let paint = task("t9", "Paint", json!({ "checklistItems": steps }));
    answer_seed(app, &effects[0], seed(scope_home(), vec![paint]));
}

fn step(id: &str, name: &str, checked: bool) -> serde_json::Value {
    json!({ "id": id, "displayName": name, "isChecked": checked })
}

#[test]
fn the_cursor_follows_its_step_when_one_above_is_deleted_elsewhere() {
    let mut app = painted();
    to_row(&mut app, DetailRow::Step(1));
    // The phone deletes "Buy paint", above "Tape".
    refreshed(&mut app, json!([step("c2", "Tape", false)]));
    assert_eq!(app.detail_row_now(), Some(DetailRow::Step(0)));
    let (_, change) = sent(&act(&mut app, Action::ToggleStep));
    assert_eq!(
        change,
        TaskChange::CheckSteps {
            steps: vec!["c2".into()],
            checked: true
        },
        "still Tape"
    );
}

#[test]
fn a_step_gone_from_under_the_cursor_is_left_alone_with_a_hint() {
    let mut app = painted();
    to_row(&mut app, DetailRow::Step(1));
    refreshed(
        &mut app,
        json!([
            step("c1", "Buy paint", true),
            step("c3", "Two coats", false)
        ]),
    );
    // The nearest step, but the next action only says what happened.
    assert_eq!(app.detail_row_now(), Some(DetailRow::Step(1)));
    assert!(act(&mut app, Action::ToggleStep).is_empty());
    assert!(
        app.banner
            .as_ref()
            .is_some_and(|banner| banner.text.contains("step changed")),
        "{:?}",
        app.banner
    );
    // Once said, the step now under the cursor is the one acted on.
    let (_, change) = sent(&act(&mut app, Action::ToggleStep));
    assert_eq!(
        change,
        TaskChange::CheckSteps {
            steps: vec!["c3".into()],
            checked: true
        }
    );

    // Every step gone: the heading, and Space does nothing.
    to_row(&mut app, DetailRow::Step(0));
    refreshed(&mut app, json!([]));
    assert_eq!(app.detail_row_now(), Some(DetailRow::Steps));
    assert!(act(&mut app, Action::ToggleStep).is_empty());
}

#[test]
fn the_cursor_keeps_a_new_step_when_it_gets_graphs_id() {
    let mut app = painted();
    refreshed(
        &mut app,
        json!([
            step("c1", "Buy paint", true),
            step("local-9", "Sand", false)
        ]),
    );
    to_row(&mut app, DetailRow::Step(1));
    refreshed(
        &mut app,
        json!([step("c1", "Buy paint", true), step("c9", "Sand", false)]),
    );
    let (_, change) = sent(&act(&mut app, Action::ToggleStep));
    assert_eq!(
        change,
        TaskChange::CheckSteps {
            steps: vec!["c9".into()],
            checked: true
        }
    );
}

#[test]
fn a_link_changed_elsewhere_is_left_alone_with_a_hint() {
    let mut app = painted();
    to_row(&mut app, DetailRow::Link);
    let effects = app.update(Msg::Event(ms_todo_protocol::Event::ResyncNeeded));
    let paint = task(
        "t9",
        "Paint",
        json!({ "linkedResources": [{ "id": "r2", "webUrl": "https://example.com/other", "applicationName": "x" }] }),
    );
    answer_seed(&mut app, &effects[0], seed(scope_home(), vec![paint]));
    assert!(act(&mut app, Action::Delete).is_empty());
    assert_eq!(app.mode, Mode::Normal, "nothing asked");
    assert!(
        app.banner
            .as_ref()
            .is_some_and(|banner| banner.text.contains("link changed"))
    );
    act(&mut app, Action::Delete);
    let (_, change) = sent(&act(&mut app, Action::Confirm));
    assert_eq!(
        change,
        TaskChange::DeleteLink {
            link: Some("r2".into())
        }
    );
}
