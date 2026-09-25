//! One due date for several tasks (rung 5d): "Set due date…" (`S`) for
//! the selection, or the task under the cursor, and "Reschedule overdue
//! to…" (`R`) for the open tasks on screen that are overdue. The day is
//! read, previewed and checked as the due-date editor's is, and one
//! `ChangeTasks` goes out, so one `u` puts every task back.

use ms_todo_core::DATE_FORMAT;
use ms_todo_nlp::read_due;
use ms_todo_protocol::{Clearable, TaskChange, TaskEdit};

use super::edit::set_value;
use super::{App, Effect, Level, LineEditor, Mode, Write, change};

impl App {
    /// `S`: ask for a due date for the selection, or the task under the
    /// cursor.
    pub(super) fn start_set_due(&mut self) {
        let targets = self.targets();
        let what = match targets.as_slice() {
            [] => return,
            [task] => format!("\"{}\"", task.title),
            many => format!("{} tasks", many.len()),
        };
        let ids = targets.iter().map(|task| task.id.clone()).collect();
        self.ask_for_due(ids, what);
    }

    /// `R`: ask for a new due date for every open task on screen that's
    /// overdue.
    pub(super) fn start_reschedule_overdue(&mut self) {
        let today = self.clock.today();
        let ids: Vec<String> = self
            .tasks
            .iter()
            .filter(|task| !task.completed && task.due.is_some_and(|due| due < today))
            .map(|task| task.id.clone())
            .collect();
        let what = match ids.len() {
            0 => {
                self.show(Level::Info, "Nothing here is overdue");
                return;
            }
            1 => "1 overdue task".to_owned(),
            count => format!("{count} overdue tasks"),
        };
        self.ask_for_due(ids, what);
    }

    fn ask_for_due(&mut self, ids: Vec<String>, what: String) {
        self.mode = Mode::SettingDue {
            ids,
            what,
            input: LineEditor::single(""),
            error: None,
        };
    }

    /// Enter: send the day typed to every task, or say why it can't be.
    pub(super) fn submit_set_due(&mut self) -> Vec<Effect> {
        let Mode::SettingDue { input, .. } = &self.mode else {
            return Vec::new();
        };
        let due = match set_value(read_due(&input.text(), &self.parse_context())) {
            Ok(Some(due)) => due,
            Ok(None) => return self.due_error("Type a day, such as tomorrow or fri"),
            Err(why) => return self.due_error(&why),
        };
        let Mode::SettingDue { ids, .. } = std::mem::replace(&mut self.mode, Mode::Normal) else {
            return Vec::new();
        };
        self.selection.clear();
        let edit = TaskEdit {
            due: Some(Clearable::Set(due.format(DATE_FORMAT).to_string())),
            ..TaskEdit::default()
        };
        vec![change(Write::Edit, ids, TaskChange::Edit(edit))]
    }

    fn due_error(&mut self, why: &str) -> Vec<Effect> {
        if let Mode::SettingDue { error, .. } = &mut self.mode {
            *error = Some(why.to_owned());
        }
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use ms_todo_protocol::{Refused, Request, ResponseData, Scope, TaskAction};
    use serde_json::json;

    use super::*;
    use crate::action::Action;
    use crate::app::edit::Field;
    use crate::app::tests::{act, answer_seed, applied, seed, seeded, task};
    use crate::app::{Msg, Tag};
    use crate::keybindings::Context;

    fn typed(app: &mut App, text: &str) {
        for ch in text.chars() {
            app.update(Msg::Char(ch));
        }
    }

    fn due_edit(effects: &[Effect]) -> (Vec<String>, TaskChange) {
        match effects {
            [
                Effect {
                    tag: Tag::Write(Write::Edit),
                    request: Request::ChangeTasks { tasks, change, .. },
                },
            ] => (tasks.clone(), change.clone()),
            other => unreachable!("not one edit: {other:?}"),
        }
    }

    fn due_on(day: &str) -> TaskChange {
        TaskChange::Edit(TaskEdit {
            due: Some(Clearable::Set(day.into())),
            ..TaskEdit::default()
        })
    }

    #[test]
    fn set_due_date_goes_to_every_selected_task_in_one_change() {
        let mut app = seeded();
        act(&mut app, Action::ToggleSelect);
        act(&mut app, Action::MoveDown);
        act(&mut app, Action::ToggleSelect);
        act(&mut app, Action::SetDue);
        assert_eq!(app.context(), Context::Prompt);
        assert!(matches!(&app.mode, Mode::SettingDue { what, .. } if what == "2 tasks"));
        // Nothing typed is an error, never a clear.
        assert!(act(&mut app, Action::Submit).is_empty());
        assert!(matches!(&app.mode, Mode::SettingDue { error: Some(_), .. }));
        typed(&mut app, "tomorrow");
        assert!(matches!(&app.mode, Mode::SettingDue { error: None, .. }));
        assert_eq!(app.date_preview(), Some(Ok("\u{2192} Fri 25 Sep".into())));
        let (tasks, change) = due_edit(&act(&mut app, Action::Submit));
        assert_eq!(tasks, ["t1", "t2"]);
        assert_eq!(change, due_on("2026-09-25"));
        assert_eq!(app.mode, Mode::Normal);
        assert!(app.selection.is_empty(), "done with once sent");
    }

    #[test]
    fn the_field_pickers_due_date_takes_the_selection_too() {
        let mut app = seeded();
        act(&mut app, Action::SelectAll);
        act(&mut app, Action::Edit);
        act(&mut app, Action::EditField(Field::Due));
        assert!(matches!(&app.mode, Mode::SettingDue { ids, .. } if ids.len() == 4));
        // Without a selection it's the one task's editor, as before.
        act(&mut app, Action::Cancel);
        app.selection.clear();
        act(&mut app, Action::Edit);
        act(&mut app, Action::EditField(Field::Due));
        assert!(matches!(
            &app.mode,
            Mode::Editing {
                field: Field::Due,
                ..
            }
        ));
    }

    #[test]
    fn reschedule_overdue_takes_only_the_open_overdue_tasks_on_screen() {
        let mut app = seeded();
        let due = |day: &str| json!({ "dueDateTime": { "dateTime": format!("{day}T00:00:00.0000000"), "timeZone": "Europe/London" } });
        let mut done_late = due("2026-09-01");
        done_late["status"] = json!("completed");
        let tasks = vec![
            task("o1", "Renew passport", due("2026-09-20")),
            task("o2", "Dentist", due("2026-09-23")),
            task("today", "Bins out", due("2026-09-24")),
            task("done", "Old thing", done_late),
            task("none", "Someday", json!({})),
        ];
        let effects = app.update(Msg::Event(ms_todo_protocol::Event::ResyncNeeded));
        answer_seed(
            &mut app,
            &effects[0],
            seed(Scope::List { id: "home".into() }, tasks),
        );
        act(&mut app, Action::RescheduleOverdue);
        assert!(matches!(&app.mode, Mode::SettingDue { what, .. } if what == "2 overdue tasks"));
        typed(&mut app, "fri");
        let (tasks, change) = due_edit(&act(&mut app, Action::Submit));
        assert_eq!(tasks, ["o1", "o2"]);
        assert_eq!(change, due_on("2026-09-25"));

        // The answer redraws the rows and says one `u` puts them back.
        let moved = vec![
            task("o1", "Renew passport", due("2026-09-25")),
            task("o2", "Dentist", due("2026-09-25")),
        ];
        app.update(Msg::Response {
            tag: Tag::Write(Write::Edit),
            result: Ok(applied(TaskAction::Edit, moved)),
        });
        assert_eq!(
            app.banner.as_ref().map(|banner| banner.text.as_str()),
            Some("Changed 2 tasks; u puts them all back")
        );
        let today = app.clock.today();
        assert!(
            app.tasks
                .iter()
                .filter(|task| !task.completed)
                .all(|task| task.due.is_none_or(|due| due >= today))
        );
        act(&mut app, Action::RescheduleOverdue);
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(
            app.banner.as_ref().map(|banner| banner.text.as_str()),
            Some("Nothing here is overdue")
        );
    }

    #[test]
    fn an_undo_that_left_tasks_alone_names_them() {
        let mut app = seeded();
        let ResponseData::Applied(mut answer) = applied(TaskAction::Undo, Vec::new()) else {
            unreachable!("applied");
        };
        answer.refused = vec![Refused {
            id: "t2".into(),
            title: "Call Sam".into(),
            reason: "has changed since (dueDateTime)".into(),
        }];
        app.update(Msg::Response {
            tag: Tag::Undo,
            result: Ok(ResponseData::Applied(answer)),
        });
        assert_eq!(
            app.banner.as_ref().map(|banner| banner.text.as_str()),
            Some("Undone, except \"Call Sam\": changed since, so left alone")
        );
    }
}
