//! "Assign to…" (`W`, rung 8d): who the selection, or the task under the
//! cursor, waits on. One name for every task, in one `ChangeTasks`, so one
//! `u` puts them all back; nothing typed clears the assignee. The daemon
//! pairs the status with it (waiting on others), as the CLI's
//! `--assignee` does.

use ms_todo_protocol::{Clearable, TaskChange, TaskEdit};

use super::{App, Effect, LineEditor, Mode, Write, change};

impl App {
    /// `W`: ask who the selection, or the task under the cursor, waits
    /// on, starting from the name they share, if they share one.
    pub(super) fn start_assign(&mut self) {
        let targets = self.targets();
        let what = match targets.as_slice() {
            [] => return,
            [task] => format!("\"{}\"", task.title),
            many => format!("{} tasks", many.len()),
        };
        let first = targets.first().and_then(|task| task.assignee.clone());
        let shared = first.filter(|name| {
            targets
                .iter()
                .all(|task| task.assignee.as_deref() == Some(name.as_str()))
        });
        self.mode = Mode::Assigning {
            ids: targets.iter().map(|task| task.id.clone()).collect(),
            what,
            input: LineEditor::single(shared.as_deref().unwrap_or_default()),
        };
    }

    /// Enter: assign every task to the name typed, or clear their assignee
    /// when it's empty.
    pub(super) fn submit_assign(&mut self) -> Vec<Effect> {
        let Mode::Assigning { ids, input, .. } = std::mem::replace(&mut self.mode, Mode::Normal)
        else {
            return Vec::new();
        };
        self.selection.clear();
        let assignee = match input.text().trim() {
            "" => Clearable::Clear,
            name => Clearable::Set(name.to_owned()),
        };
        let edit = TaskEdit {
            assignee: Some(assignee),
            ..TaskEdit::default()
        };
        vec![change(Write::Edit, ids, TaskChange::Edit(edit))]
    }
}

#[cfg(test)]
mod tests {
    use ms_todo_protocol::{Request, TaskAction};
    use serde_json::json;

    use super::*;
    use crate::action::Action;
    use crate::app::edit::Field;
    use crate::app::tests::{act, applied, seeded, task};
    use crate::app::{Msg, Pane, Tag};
    use crate::keybindings::Context;

    fn typed(app: &mut App, text: &str) {
        for ch in text.chars() {
            app.update(Msg::Char(ch));
        }
    }

    fn sent(effects: &[Effect]) -> (Vec<String>, TaskChange) {
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

    fn assign(assignee: Clearable<String>) -> TaskChange {
        TaskChange::Edit(TaskEdit {
            assignee: Some(assignee),
            ..TaskEdit::default()
        })
    }

    #[test]
    fn assign_to_goes_to_every_selected_task_in_one_change() {
        let mut app = seeded();
        act(&mut app, Action::ToggleSelect);
        act(&mut app, Action::MoveDown);
        act(&mut app, Action::ToggleSelect);
        act(&mut app, Action::Assign);
        assert_eq!(app.context(), Context::Prompt);
        assert!(matches!(&app.mode, Mode::Assigning { what, .. } if what == "2 tasks"));
        typed(&mut app, " Sam ");
        let (tasks, change) = sent(&act(&mut app, Action::Submit));
        assert_eq!(tasks, ["t1", "t2"]);
        assert_eq!(change, assign(Clearable::Set("Sam".into())));
        assert_eq!(app.mode, Mode::Normal);
        assert!(app.selection.is_empty());
    }

    #[test]
    fn the_prompt_starts_from_the_shared_name_and_empty_clears_it() {
        let mut app = seeded();
        let sam = task(
            "t1",
            "Get the quote",
            json!({ "extensions": [{ "assignee": "Sam" }] }),
        );
        app.update(Msg::Response {
            tag: Tag::Write(Write::Edit),
            result: Ok(applied(TaskAction::Edit, vec![sam])),
        });
        app.task_index = 0;
        act(&mut app, Action::Assign);
        assert!(matches!(&app.mode, Mode::Assigning { input, .. } if input.text() == "Sam"));
        app.update(Msg::Key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('u'),
            crossterm::event::KeyModifiers::CONTROL,
        )));
        let (tasks, change) = sent(&act(&mut app, Action::Submit));
        assert_eq!(tasks, ["t1"]);
        assert_eq!(change, assign(Clearable::Clear));
    }

    #[test]
    fn e_on_the_assignee_field_edits_it_directly() {
        let mut app = seeded();
        app.focus = Pane::Detail;
        for _ in 0..4 {
            act(&mut app, Action::MoveDown);
        }
        assert_eq!(
            app.detail_row,
            crate::app::steps::DetailRow::Field(Field::Assignee)
        );
        act(&mut app, Action::Edit);
        assert!(matches!(
            &app.mode,
            Mode::Editing {
                field: Field::Assignee,
                ..
            }
        ));
        typed(&mut app, "ada@example.com");
        let (tasks, change) = sent(&act(&mut app, Action::Submit));
        assert_eq!(tasks, ["t1"]);
        assert_eq!(change, assign(Clearable::Set("ada@example.com".into())));
    }

    #[test]
    fn the_pickers_assignee_takes_the_selection() {
        let mut app = seeded();
        act(&mut app, Action::SelectAll);
        act(&mut app, Action::Edit);
        act(&mut app, Action::EditField(Field::Assignee));
        assert!(matches!(&app.mode, Mode::Assigning { ids, .. } if ids.len() == 4));
    }
}
