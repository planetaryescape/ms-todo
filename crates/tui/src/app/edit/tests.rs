use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ms_todo_protocol::{Request, ResponseData, TaskAction};
use serde_json::json;

use super::*;
use crate::action::Action;
use crate::app::tests::{act, applied, seeded, task, titles};
use crate::app::{Msg, Tag};
use crate::keybindings::{Context, key_for, resolve};

fn type_text(app: &mut App, text: &str) {
    for ch in text.chars() {
        app.update(Msg::Char(ch));
    }
}

fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> Vec<Effect> {
    app.update(Msg::Key(KeyEvent::new(code, modifiers)))
}

/// What's in the editor, and where its cursor is.
fn typed(app: &App) -> (String, (usize, usize)) {
    match &app.mode {
        Mode::Editing { input, .. } => (input.text(), input.cursor()),
        other => unreachable!("not editing: {other:?}"),
    }
}

/// Erase what the field started as and type `text`.
fn retype(app: &mut App, text: &str) {
    press(app, KeyCode::End, KeyModifiers::NONE);
    press(app, KeyCode::Char('u'), KeyModifiers::CONTROL);
    type_text(app, text);
}

fn edit_sent(effects: &[Effect]) -> (Vec<String>, TaskEdit) {
    match effects {
        [
            Effect {
                tag: Tag::Write(Write::Edit),
                request:
                    Request::ChangeTasks {
                        tasks,
                        change: TaskChange::Edit(edit),
                        dry_run: false,
                        ..
                    },
            },
        ] => (tasks.clone(), edit.clone()),
        other => unreachable!("not one edit: {other:?}"),
    }
}

/// `e`, then the field's key in the picker.
fn pick(app: &mut App, field: Field) -> Vec<Effect> {
    act(app, Action::Edit);
    assert_eq!(app.context(), Context::Fields);
    let bound = key_for(Context::Fields, Action::EditField(field)).expect("a picker key");
    let key = KeyEvent::new(
        KeyCode::Char(bound.chars().next().expect("one key")),
        KeyModifiers::NONE,
    );
    let action = resolve(Context::Fields, &key).expect("a picker key");
    assert_eq!(action, Action::EditField(field));
    act(app, action)
}

/// Move the detail pane's cursor to `field`, from the title.
fn to_field(app: &mut App, field: Field) {
    act(app, Action::FocusRight);
    act(app, Action::JumpTop);
    while app.detail_row != DetailRow::Field(field) {
        act(app, Action::MoveDown);
    }
}

#[test]
fn e_opens_the_picker_and_t_edits_the_title_in_one_change_tasks_edit() {
    let mut app = seeded();
    assert!(act(&mut app, Action::Edit).is_empty());
    assert_eq!(app.mode, Mode::ChoosingField { id: "t1".into() });
    act(&mut app, Action::EditField(Field::Title));
    assert_eq!(
        app.mode,
        Mode::Editing {
            id: "t1".into(),
            field: Field::Title,
            input: LineEditor::single("Pay rent"),
            error: None
        }
    );
    // The cursor starts at the end of the value.
    assert_eq!(typed(&app).1, (0, 8));
    assert_eq!(app.focus, Pane::Detail);
    retype(&mut app, "Pay the rent ");
    let (tasks, edit) = edit_sent(&act(&mut app, Action::Submit));
    assert_eq!(tasks, ["t1"]);
    assert_eq!(
        edit,
        TaskEdit {
            title: Some("Pay the rent".into()),
            ..TaskEdit::default()
        }
    );
    assert_eq!(app.mode, Mode::Normal);

    // The answer is drawn at once, pending.
    let renamed = task(
        "t1",
        "Pay the rent",
        json!({ "importance": "high", "sync_state": "pending" }),
    );
    app.update(Msg::Response {
        tag: Tag::Write(Write::Edit),
        result: Ok(applied(TaskAction::Edit, vec![renamed])),
    });
    assert_eq!(titles(&app)[0], "Pay the rent");
}

#[test]
fn the_cursor_moves_and_text_goes_in_mid_string() {
    let mut app = seeded();
    pick(&mut app, Field::Title);
    // "Pay rent": back over "rent", then type there.
    press(&mut app, KeyCode::Char('b'), KeyModifiers::ALT);
    assert_eq!(typed(&app), ("Pay rent".into(), (0, 4)));
    type_text(&mut app, "the ");
    assert_eq!(typed(&app), ("Pay the rent".into(), (0, 8)));
    press(&mut app, KeyCode::Home, KeyModifiers::NONE);
    press(&mut app, KeyCode::Right, KeyModifiers::NONE);
    press(&mut app, KeyCode::Right, KeyModifiers::NONE);
    press(&mut app, KeyCode::Right, KeyModifiers::NONE);
    act(&mut app, Action::Backspace);
    assert_eq!(typed(&app), ("Pa the rent".into(), (0, 2)));
    press(&mut app, KeyCode::Delete, KeyModifiers::NONE);
    assert_eq!(typed(&app), ("Pathe rent".into(), (0, 2)));
    press(&mut app, KeyCode::Char('e'), KeyModifiers::CONTROL);
    assert_eq!(typed(&app).1, (0, 10));
    let (_, edit) = edit_sent(&act(&mut app, Action::Submit));
    assert_eq!(edit.title.as_deref(), Some("Pathe rent"));
}

#[test]
fn word_and_line_deletes() {
    let mut app = seeded();
    pick(&mut app, Field::Title);
    type_text(&mut app, " today");
    press(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL);
    assert_eq!(typed(&app).0, "Pay rent ");
    press(&mut app, KeyCode::Left, KeyModifiers::CONTROL);
    press(&mut app, KeyCode::Char('k'), KeyModifiers::CONTROL);
    assert_eq!(typed(&app).0, "Pay ");
    press(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
    assert_eq!(typed(&app), (String::new(), (0, 0)));
}

#[test]
fn the_picker_goes_to_each_field() {
    for (field, starts_as) in [
        (Field::Title, "Pay rent"),
        (Field::Due, "2026-10-01"),
        (Field::Reminder, ""),
        (Field::Notes, ""),
    ] {
        let mut app = seeded();
        // From the list pane.
        assert_eq!(app.focus, Pane::Tasks);
        pick(&mut app, field);
        let Mode::Editing {
            id,
            field: editing,
            input,
            ..
        } = &app.mode
        else {
            unreachable!("{field:?} isn't being edited: {:?}", app.mode);
        };
        assert_eq!((id.as_str(), *editing), ("t1", field));
        assert_eq!(input.text(), starts_as, "{field:?}");
        assert_eq!(
            app.context(),
            if field == Field::Notes {
                Context::Notes
            } else {
                Context::Prompt
            }
        );
        assert_eq!(
            (app.focus, app.detail_row),
            (Pane::Detail, DetailRow::Field(field))
        );
        act(&mut app, Action::Cancel);
        assert_eq!(app.mode, Mode::Normal);
    }
    // Importance asks for a level; Esc leaves the picker.
    let mut app = seeded();
    pick(&mut app, Field::Importance);
    assert_eq!(app.mode, Mode::ChoosingImportance { id: "t1".into() });
    assert_eq!(app.context(), Context::Importance);
    act(&mut app, Action::Cancel);
    act(&mut app, Action::Edit);
    assert!(act(&mut app, Action::Cancel).is_empty());
    assert_eq!(app.mode, Mode::Normal);
}

#[test]
fn e_from_the_detail_pane_opens_the_picker_and_enter_edits_the_field_under_the_cursor() {
    let mut app = seeded();
    to_field(&mut app, Field::Due);
    act(&mut app, Action::Edit);
    assert_eq!(app.context(), Context::Fields);
    act(&mut app, Action::Cancel);
    act(&mut app, Action::EditHere);
    assert!(matches!(
        &app.mode,
        Mode::Editing {
            field: Field::Due,
            ..
        }
    ));
}

#[test]
fn importance_by_level_sends_one_edit_and_the_same_level_sends_nothing() {
    for (key, expected) in [
        ('1', None),
        ('2', Some(Importance::Normal)),
        ('3', Some(Importance::Normal)),
        ('4', Some(Importance::Low)),
        ('h', None),
        ('n', Some(Importance::Normal)),
        ('l', Some(Importance::Low)),
    ] {
        let mut app = seeded();
        // "Pay rent" is high.
        pick(&mut app, Field::Importance);
        let pressed = KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE);
        let action = resolve(Context::Importance, &pressed).expect("a level key");
        let effects = act(&mut app, action);
        match expected {
            None => assert!(effects.is_empty(), "{key}"),
            Some(level) => {
                let (tasks, edit) = edit_sent(&effects);
                assert_eq!(tasks, ["t1"]);
                assert_eq!(
                    edit,
                    TaskEdit {
                        importance: Some(level),
                        ..TaskEdit::default()
                    },
                    "{key}"
                );
            }
        }
        assert_eq!(app.mode, Mode::Normal);
    }
}

#[test]
fn shift_i_cycles_importance_in_one_edit() {
    let mut app = seeded();
    act(&mut app, Action::Edit);
    let shift_i = KeyEvent::new(KeyCode::Char('I'), KeyModifiers::SHIFT);
    let action = resolve(Context::Fields, &shift_i).expect("I");
    assert_eq!(action, Action::CycleImportance);
    // High goes round to low.
    let (tasks, edit) = edit_sent(&act(&mut app, action));
    assert_eq!(tasks, ["t1"]);
    assert_eq!(edit.importance, Some(Importance::Low));
    assert_eq!(app.mode, Mode::Normal);

    // "Call Sam" is normal: up to high.
    act(&mut app, Action::MoveDown);
    let (tasks, edit) = edit_sent(&act(&mut app, Action::CycleImportance));
    assert_eq!(tasks, ["t2"]);
    assert_eq!(edit.importance, Some(Importance::High));
}

#[test]
fn date_shortcuts_resolve_before_sending() {
    // The test clock is Thursday 24 September 2026, 13:00 in London.
    for (field, input, due, reminder) in [
        (Field::Due, "tomorrow", Some("2026-09-25"), None),
        (Field::Due, "fri", Some("2026-09-25"), None),
        (
            Field::Due,
            "three days from today",
            Some("2026-09-27"),
            None,
        ),
        (Field::Due, "+2w", Some("2026-10-08"), None),
        (Field::Due, "in 2 days", Some("2026-09-26"), None),
        (Field::Due, "12 oct", Some("2026-10-12"), None),
        (Field::Reminder, "17:30", None, Some("2026-09-24T17:30")),
        (Field::Reminder, "9:00", None, Some("2026-09-25T09:00")),
        // A day alone is 09:00 on it, as To Do's "Tomorrow" is.
        (Field::Reminder, "in 2 days", None, Some("2026-09-26T09:00")),
        (Field::Reminder, "tomorrow", None, Some("2026-09-25T09:00")),
        (
            Field::Reminder,
            "next mon 9am",
            None,
            Some("2026-09-28T09:00"),
        ),
    ] {
        let mut app = seeded();
        pick(&mut app, field);
        retype(&mut app, input);
        let (_, edit) = edit_sent(&act(&mut app, Action::Submit));
        assert_eq!(
            edit.due,
            due.map(|due| Clearable::Set(due.to_owned())),
            "{input}"
        );
        assert_eq!(
            edit.reminder,
            reminder.map(|at| Clearable::Set(at.to_owned())),
            "{input}"
        );
    }
}

#[test]
fn the_resolved_date_is_shown_while_typing() {
    let mut app = seeded();
    pick(&mut app, Field::Due);
    retype(&mut app, "next fri");
    assert_eq!(app.date_preview(), Some(Ok("\u{2192} Fri 2 Oct".into())));
    retype(&mut app, "yesterday");
    assert_eq!(
        app.date_preview(),
        Some(Ok("\u{2192} Wed 23 Sep, in the past".into()))
    );
    retype(&mut app, "");
    assert_eq!(
        app.date_preview(),
        Some(Ok("\u{2192} none: clears it".into()))
    );
    retype(&mut app, "tomo");
    assert!(matches!(app.date_preview(), Some(Err(why)) if why.contains("tomo")));
    act(&mut app, Action::Cancel);
    pick(&mut app, Field::Reminder);
    retype(&mut app, "in 2 days");
    assert_eq!(
        app.date_preview(),
        Some(Ok("\u{2192} Sat 26 Sep 09:00".into()))
    );
    act(&mut app, Action::Cancel);
    pick(&mut app, Field::Title);
    assert_eq!(app.date_preview(), None);
}

#[test]
fn an_empty_value_or_a_dash_clears_a_date() {
    let mut app = seeded();
    pick(&mut app, Field::Due);
    retype(&mut app, "");
    let (_, edit) = edit_sent(&act(&mut app, Action::Submit));
    assert_eq!(edit.due, Some(Clearable::Clear));

    let mut app = seeded();
    pick(&mut app, Field::Due);
    retype(&mut app, "-");
    let (_, edit) = edit_sent(&act(&mut app, Action::Submit));
    assert_eq!(edit.due, Some(Clearable::Clear));

    // No reminder to clear: nothing to send.
    pick(&mut app, Field::Reminder);
    assert!(act(&mut app, Action::Submit).is_empty());
    assert_eq!(app.mode, Mode::Normal);
}

#[test]
fn invalid_input_shows_why_and_sends_nothing_until_fixed() {
    let mut app = seeded();
    for (field, bad) in [
        (Field::Title, "   "),
        (Field::Due, "2026-13-40"),
        (Field::Due, "soonish"),
        (Field::Due, "tomorrow 9am"),
        (Field::Reminder, "2026-10-01 9 in the morning"),
    ] {
        pick(&mut app, field);
        retype(&mut app, bad);
        assert!(
            act(&mut app, Action::Submit).is_empty(),
            "{field:?} {bad:?}"
        );
        let Mode::Editing { error, .. } = &app.mode else {
            unreachable!("still editing after {bad:?}");
        };
        assert!(error.is_some(), "{field:?} {bad:?}");
        // A move keeps the error; typing takes it away.
        press(&mut app, KeyCode::Left, KeyModifiers::NONE);
        assert!(matches!(&app.mode, Mode::Editing { error: Some(_), .. }));
        type_text(&mut app, "x");
        assert!(matches!(&app.mode, Mode::Editing { error: None, .. }));
        act(&mut app, Action::Cancel);
    }
}

#[test]
fn notes_take_several_lines_and_save_with_ctrl_s() {
    let mut app = seeded();
    pick(&mut app, Field::Notes);
    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    let ctrl_s = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
    let alt_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT);
    assert_eq!(resolve(Context::Notes, &enter), Some(Action::Newline));
    assert_eq!(resolve(Context::Notes, &ctrl_s), Some(Action::Submit));
    assert_eq!(resolve(Context::Notes, &alt_enter), Some(Action::Submit));
    type_text(&mut app, "Standing order");
    act(&mut app, Action::Newline);
    type_text(&mut app, "on the 1st");
    assert_eq!(typed(&app), ("Standing order\non the 1st".into(), (1, 10)));
    // Up to the first line, and on at its end.
    press(&mut app, KeyCode::Up, KeyModifiers::NONE);
    press(&mut app, KeyCode::End, KeyModifiers::NONE);
    type_text(&mut app, ",");
    let (_, edit) = edit_sent(&act(&mut app, Action::Submit));
    assert_eq!(edit.body.as_deref(), Some("Standing order,\non the 1st"));
    assert_eq!(app.mode, Mode::Normal);

    // On one line, Enter saves: a title has no second line.
    pick(&mut app, Field::Title);
    assert_eq!(resolve(Context::Prompt, &enter), Some(Action::Submit));
}

#[test]
fn escape_cancels_and_an_unchanged_value_sends_nothing() {
    let mut app = seeded();
    pick(&mut app, Field::Title);
    type_text(&mut app, " now");
    assert!(act(&mut app, Action::Cancel).is_empty());
    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(titles(&app)[0], "Pay rent");

    pick(&mut app, Field::Title);
    assert!(act(&mut app, Action::Submit).is_empty());
    assert_eq!(app.mode, Mode::Normal);
}

#[test]
fn the_palette_edits_a_field_directly() {
    let mut app = seeded();
    act(&mut app, Action::Palette);
    type_text(&mut app, "edit due");
    act(&mut app, Action::Submit);
    assert!(matches!(
        &app.mode,
        Mode::Editing {
            field: Field::Due,
            ..
        }
    ));
    act(&mut app, Action::Cancel);

    act(&mut app, Action::Palette);
    type_text(&mut app, "cycle imp");
    let (_, edit) = edit_sent(&act(&mut app, Action::Submit));
    assert_eq!(edit.importance, Some(Importance::Low));
}

#[test]
fn html_notes_left_as_they_are_are_not_rewritten_as_text() {
    let mut app = seeded();
    let notes = task(
        "t1",
        "Pay rent",
        json!({ "body": { "content": "<p>Call <b>Sam</b></p>", "contentType": "html" } }),
    );
    let effects = app.update(Msg::Event(ms_todo_protocol::Event::ResyncNeeded));
    crate::app::tests::answer_seed(
        &mut app,
        &effects[0],
        crate::app::tests::seed(crate::app::tests::scope_home(), vec![notes]),
    );
    pick(&mut app, Field::Notes);
    assert_eq!(typed(&app).0, "Call Sam");
    assert!(act(&mut app, Action::Submit).is_empty());
}

#[test]
fn a_task_gone_while_typing_or_picking_is_not_edited() {
    let gone = |app: &mut App| {
        // Deleted on the phone meanwhile.
        let effects = app.update(Msg::Event(ms_todo_protocol::Event::ResyncNeeded));
        let mut tasks = crate::app::tests::home_tasks();
        tasks.remove(0);
        app.update(Msg::Response {
            tag: effects[0].tag,
            result: Ok(ResponseData::Seed(crate::app::tests::seed(
                crate::app::tests::scope_home(),
                tasks,
            ))),
        });
    };
    let mut app = seeded();
    pick(&mut app, Field::Title);
    type_text(&mut app, "!");
    gone(&mut app);
    assert!(act(&mut app, Action::Submit).is_empty());
    assert_eq!(app.mode, Mode::Normal);
    assert!(app.banner.is_some());

    // The picker keeps the task it was opened on, by ID: the row under
    // the cursor is now another task, which isn't touched.
    let mut app = seeded();
    act(&mut app, Action::Edit);
    gone(&mut app);
    assert!(act(&mut app, Action::CycleImportance).is_empty());
    assert_eq!(app.mode, Mode::Normal);
    assert!(app.banner.is_some());
}
