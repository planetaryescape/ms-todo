//! Editing one field of the selected task in the detail pane: what the
//! field starts as, and what's typed is checked before anything is sent,
//! so invalid input costs nothing but an inline error.

use chrono::{NaiveDate, NaiveDateTime};
use ms_todo_core::DATE_FORMAT;
use ms_todo_protocol::{Clearable, Importance, TaskChange, TaskEdit};

use super::{App, Effect, Level, Mode, Pane, Task, Write, change};

/// The reminder as typed, in local time (the daemon's format).
const REMINDER_FORMAT: &str = "%Y-%m-%dT%H:%M";

/// A field the detail pane edits, in the order it shows them: j and k
/// move through them in this order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Field {
    #[default]
    Title,
    Due,
    Reminder,
    Importance,
    Notes,
}

impl Field {
    pub const ALL: [Self; 5] = [
        Self::Title,
        Self::Due,
        Self::Reminder,
        Self::Importance,
        Self::Notes,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Title => "Title",
            Self::Due => "Due",
            Self::Importance => "Importance",
            Self::Reminder => "Reminder",
            Self::Notes => "Notes",
        }
    }

    /// What to type, shown under the field while it's edited.
    pub fn format_hint(self) -> &'static str {
        match self {
            Self::Title => "the new title",
            Self::Due => "YYYY-MM-DD, or empty to clear",
            Self::Importance => "low, normal or high",
            Self::Reminder => "YYYY-MM-DDTHH:MM in local time, or empty to clear",
            Self::Notes => "plain text, or empty to clear",
        }
    }

    /// The field's value as it's typed, to start editing from.
    pub fn current(self, task: &Task) -> String {
        match self {
            Self::Title => task.title.clone(),
            Self::Due => task
                .due
                .map(|due| due.format(DATE_FORMAT).to_string())
                .unwrap_or_default(),
            Self::Importance => importance_name(task.importance).to_owned(),
            Self::Reminder => task
                .reminder
                .map(|at| at.format(REMINDER_FORMAT).to_string())
                .unwrap_or_default(),
            Self::Notes => task.notes().unwrap_or_default(),
        }
    }

    /// The field `by` rows away, stopping at the first and last.
    pub fn step(self, by: isize) -> Self {
        let at = Self::ALL
            .iter()
            .position(|field| *field == self)
            .unwrap_or(0);
        let last = Self::ALL.len() - 1;
        Self::ALL[at.saturating_add_signed(by).min(last)]
    }
}

pub fn importance_name(importance: Importance) -> &'static str {
    match importance {
        Importance::Low => "low",
        Importance::Normal => "normal",
        Importance::High => "high",
    }
}

/// The edit `text` makes to `field` of `task`: `None` when it changes
/// nothing, and why not when it can't be sent.
pub fn parse(field: Field, text: &str, task: &Task) -> Result<Option<TaskEdit>, String> {
    let typed = text.trim();
    let edit = match field {
        Field::Title => {
            if typed.is_empty() {
                return Err("A title can't be empty".into());
            }
            (typed != task.title).then(|| TaskEdit {
                title: Some(typed.to_owned()),
                ..TaskEdit::default()
            })
        }
        Field::Due => {
            let due = match typed {
                "" => None,
                _ => Some(NaiveDate::parse_from_str(typed, DATE_FORMAT).map_err(|_| {
                    "Not a date: type YYYY-MM-DD, or clear it to remove the due date".to_owned()
                })?),
            };
            (due != task.due).then(|| TaskEdit {
                due: Some(clearable(
                    due.map(|due| due.format(DATE_FORMAT).to_string()),
                )),
                ..TaskEdit::default()
            })
        }
        Field::Importance => {
            let importance = match typed.to_ascii_lowercase().as_str() {
                "low" => Importance::Low,
                "normal" => Importance::Normal,
                "high" => Importance::High,
                _ => return Err("Type low, normal or high".into()),
            };
            (importance != task.importance).then(|| TaskEdit {
                importance: Some(importance),
                ..TaskEdit::default()
            })
        }
        Field::Reminder => {
            let at = match typed {
                "" => None,
                _ => Some(
                    NaiveDateTime::parse_from_str(typed, REMINDER_FORMAT).map_err(|_| {
                        "Not a date and time: type YYYY-MM-DDTHH:MM, or clear it to remove \
                         the reminder"
                            .to_owned()
                    })?,
                ),
            };
            (at != task.reminder).then(|| TaskEdit {
                reminder: Some(clearable(
                    at.map(|at| at.format(REMINDER_FORMAT).to_string()),
                )),
                ..TaskEdit::default()
            })
        }
        // Compared as rendered, so html notes left alone stay html.
        Field::Notes => (typed != task.notes().unwrap_or_default()).then(|| TaskEdit {
            body: Some(typed.to_owned()),
            ..TaskEdit::default()
        }),
    };
    Ok(edit)
}

fn clearable(value: Option<String>) -> Clearable<String> {
    value.map_or(Clearable::Clear, Clearable::Set)
}

impl App {
    /// `e`, or Enter in the detail pane: edit the field under the detail
    /// pane's cursor.
    pub(super) fn start_edit(&mut self) -> Vec<Effect> {
        if self.still_loading() {
            return Vec::new();
        }
        let Some(task) = self.selected() else {
            return Vec::new();
        };
        let field = self.detail_field;
        self.mode = Mode::Editing {
            id: task.id.clone(),
            field,
            text: field.current(task),
            error: None,
        };
        self.focus = Pane::Detail;
        Vec::new()
    }

    /// Enter while editing: send the change, or say why it can't be sent
    /// and keep editing.
    pub(super) fn submit_edit(&mut self) -> Vec<Effect> {
        let Mode::Editing {
            id, field, text, ..
        } = &self.mode
        else {
            return Vec::new();
        };
        let (id, field, text) = (id.clone(), *field, text.clone());
        // The rows may have changed while typing: only a task still on
        // screen, found by its ID, is edited.
        let task = self
            .tasks
            .iter()
            .find(|task| task.id == id)
            .filter(|_| !self.loading());
        let Some(task) = task else {
            self.mode = Mode::Normal;
            self.show(Level::Info, "That task is gone; nothing was changed");
            return Vec::new();
        };
        match parse(field, &text, task) {
            Err(why) => {
                if let Mode::Editing { error, .. } = &mut self.mode {
                    *error = Some(why);
                }
                Vec::new()
            }
            Ok(None) => {
                self.mode = Mode::Normal;
                Vec::new()
            }
            Ok(Some(edit)) => {
                self.mode = Mode::Normal;
                vec![change(Write::Edit, vec![id], TaskChange::Edit(edit))]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use ms_todo_protocol::{Request, ResponseData, TaskAction};
    use serde_json::json;

    use super::*;
    use crate::action::Action;
    use crate::app::tests::{act, applied, seeded, task, titles};
    use crate::app::{Msg, Tag};

    fn type_text(app: &mut App, text: &str) {
        for ch in text.chars() {
            app.update(Msg::Char(ch));
        }
    }

    /// Erase what the field started as and type `text`.
    fn retype(app: &mut App, text: &str) {
        while matches!(&app.mode, Mode::Editing { text, .. } if !text.is_empty()) {
            act(app, Action::Backspace);
        }
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

    /// Move the detail pane's cursor to `field`, from the title.
    fn to_field(app: &mut App, field: Field) {
        act(app, Action::FocusRight);
        act(app, Action::JumpTop);
        while app.detail_field != field {
            act(app, Action::MoveDown);
        }
    }

    #[test]
    fn e_edits_the_title_and_sends_one_change_tasks_edit() {
        let mut app = seeded();
        assert!(act(&mut app, Action::Edit).is_empty());
        assert_eq!(
            app.mode,
            Mode::Editing {
                id: "t1".into(),
                field: Field::Title,
                text: "Pay rent".into(),
                error: None
            }
        );
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
    fn enter_in_the_detail_pane_edits_the_field_under_the_cursor() {
        let mut app = seeded();
        to_field(&mut app, Field::Due);
        act(&mut app, Action::Edit);
        assert!(
            matches!(&app.mode, Mode::Editing { field: Field::Due, text, .. } if text == "2026-10-01")
        );
        retype(&mut app, "2026-10-05");
        let (_, edit) = edit_sent(&act(&mut app, Action::Submit));
        assert_eq!(edit.due, Some(Clearable::Set("2026-10-05".into())));
        // The cursor stays on the field, for the next one.
        assert_eq!((app.focus, app.detail_field), (Pane::Detail, Field::Due));

        // Down the pane, in the order it shows the fields.
        act(&mut app, Action::MoveDown);
        act(&mut app, Action::Edit);
        retype(&mut app, "2026-10-01T09:30");
        let (_, edit) = edit_sent(&act(&mut app, Action::Submit));
        assert_eq!(
            edit.reminder,
            Some(Clearable::Set("2026-10-01T09:30".into()))
        );

        act(&mut app, Action::MoveDown);
        act(&mut app, Action::Edit);
        assert!(
            matches!(&app.mode, Mode::Editing { field: Field::Importance, text, .. } if text == "high")
        );
        retype(&mut app, "Low");
        let (_, edit) = edit_sent(&act(&mut app, Action::Submit));
        assert_eq!(edit.importance, Some(Importance::Low));

        act(&mut app, Action::MoveDown);
        act(&mut app, Action::Edit);
        type_text(&mut app, "Standing order on the 1st");
        let (_, edit) = edit_sent(&act(&mut app, Action::Submit));
        assert_eq!(edit.body.as_deref(), Some("Standing order on the 1st"));
        // At the last field, Down stays.
        act(&mut app, Action::MoveDown);
        assert_eq!(app.detail_field, Field::Notes);
    }

    #[test]
    fn an_empty_due_date_or_reminder_clears_it() {
        let mut app = seeded();
        to_field(&mut app, Field::Due);
        act(&mut app, Action::Edit);
        retype(&mut app, "");
        let (_, edit) = edit_sent(&act(&mut app, Action::Submit));
        assert_eq!(edit.due, Some(Clearable::Clear));

        // No reminder to clear: nothing to send.
        to_field(&mut app, Field::Reminder);
        act(&mut app, Action::Edit);
        assert!(act(&mut app, Action::Submit).is_empty());
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn invalid_input_shows_why_and_sends_nothing_until_fixed() {
        let mut app = seeded();
        for (field, bad) in [
            (Field::Title, "   "),
            (Field::Due, "2026-13-40"),
            (Field::Due, "tomorrow"),
            (Field::Importance, "urgent"),
            (Field::Reminder, "2026-10-01 9am"),
        ] {
            to_field(&mut app, field);
            act(&mut app, Action::Edit);
            retype(&mut app, bad);
            assert!(
                act(&mut app, Action::Submit).is_empty(),
                "{field:?} {bad:?}"
            );
            let Mode::Editing { error, .. } = &app.mode else {
                unreachable!("still editing after {bad:?}");
            };
            assert!(error.is_some(), "{field:?} {bad:?}");
            // Typing again takes the error away.
            type_text(&mut app, "x");
            assert!(matches!(&app.mode, Mode::Editing { error: None, .. }));
            act(&mut app, Action::Cancel);
        }
    }

    #[test]
    fn escape_cancels_and_an_unchanged_value_sends_nothing() {
        let mut app = seeded();
        act(&mut app, Action::Edit);
        type_text(&mut app, " now");
        assert!(act(&mut app, Action::Cancel).is_empty());
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(titles(&app)[0], "Pay rent");

        act(&mut app, Action::Edit);
        assert!(act(&mut app, Action::Submit).is_empty());
        assert_eq!(app.mode, Mode::Normal);
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
        to_field(&mut app, Field::Notes);
        act(&mut app, Action::Edit);
        assert!(matches!(&app.mode, Mode::Editing { text, .. } if text == "Call Sam"));
        assert!(act(&mut app, Action::Submit).is_empty());
    }

    #[test]
    fn a_task_gone_while_typing_is_not_edited() {
        let mut app = seeded();
        act(&mut app, Action::Edit);
        type_text(&mut app, "!");
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
        assert!(act(&mut app, Action::Submit).is_empty());
        assert_eq!(app.mode, Mode::Normal);
        assert!(app.banner.is_some());
    }
}
