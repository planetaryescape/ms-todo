//! Nag reminders in the TUI (rung 9b): `n` sets the selection, or the
//! task under the cursor, to nag every [`DEFAULT_MINUTES`] once its
//! reminder is due, or stops them all when they all nag already. Another
//! interval is `tasks nag --every` in the CLI.

use ms_todo_protocol::{Applied, Clearable, TaskChange, TaskEdit};

use super::task::Task;
use super::{App, Effect, Level, Write, change};

/// What `n` sets: often enough to be hard to ignore, not so often that
/// it's noise.
pub(crate) const DEFAULT_MINUTES: u32 = 15;

impl App {
    pub(super) fn toggle_nag(&mut self) -> Vec<Effect> {
        let targets = self.targets();
        if targets.is_empty() {
            return Vec::new();
        }
        let quiet: Vec<_> = targets
            .iter()
            .copied()
            .filter(|task| task.nag.is_none())
            .collect();
        let (tasks, nag) = if quiet.is_empty() {
            (targets.clone(), Clearable::Clear)
        } else {
            // Said here, before asking: the daemon would refuse it anyway.
            if let Some(task) = quiet.iter().find(|task| task.reminder.is_none()) {
                let text = format!(
                    "\"{}\" has no reminder, and a nag starts at the reminder; set one first (e)",
                    ms_todo_core::one_line_safe(&task.title)
                );
                self.show(Level::Error, &text);
                return Vec::new();
            }
            (quiet, Clearable::Set(DEFAULT_MINUTES))
        };
        let ids = tasks.iter().map(|task| task.id.clone()).collect();
        let effect = change(Write::Nag, ids, nag_edit(nag));
        self.selection.clear();
        vec![effect]
    }

    /// Say what `n` did: the rows show it already.
    pub(super) fn nag_changed(&mut self, applied: &Applied) {
        let nagging = applied
            .items
            .iter()
            .filter_map(Task::from_entity)
            .any(|task| task.nag.is_some());
        let what = match applied.items.as_slice() {
            [] => return,
            [task] => format!(
                "\"{}\"",
                task.get("title")
                    .and_then(|title| title.as_str())
                    .unwrap_or_default()
            ),
            many => format!("{} tasks", many.len()),
        };
        let text = if nagging {
            format!(
                "Nagging about {what} every {DEFAULT_MINUTES}m once the reminder is due; u stops"
            )
        } else {
            format!("Stopped nagging about {what}; u starts again")
        };
        self.show(Level::Info, &text);
    }
}

fn nag_edit(nag: Clearable<u32>) -> TaskChange {
    TaskChange::Edit(TaskEdit {
        nag: Some(nag),
        ..TaskEdit::default()
    })
}

#[cfg(test)]
mod tests {
    use ms_todo_protocol::{Clearable, Request, TaskChange};
    use serde_json::json;

    use crate::action::Action;
    use crate::app::tests::{act, answer_seed, clock, scope_home, seed, task};
    use crate::app::{App, Level, Msg, Pane};
    use crate::glyphs::UNICODE;

    fn with(tasks: Vec<ms_todo_protocol::Entity>) -> App {
        let mut app = App::new(UNICODE, clock());
        let effects = app.update(Msg::Connected);
        answer_seed(&mut app, &effects[0], seed(scope_home(), tasks));
        app.focus = Pane::Tasks;
        app
    }

    fn reminder() -> serde_json::Value {
        json!({ "reminderDateTime": { "dateTime": "2026-09-24T09:00:00.0000000", "timeZone": "UTC" }, "isReminderOn": true })
    }

    fn nag_of(effects: &[crate::app::Effect]) -> Option<Clearable<u32>> {
        match &effects[0].request {
            Request::ChangeTasks {
                change: TaskChange::Edit(edit),
                ..
            } => edit.nag.clone(),
            other => unreachable!("{other:?}"),
        }
    }

    #[test]
    fn n_starts_a_nag_on_a_task_with_a_reminder_and_stops_it_again() {
        let mut app = with(vec![task("t1", "Call mum", reminder())]);
        let effects = act(&mut app, Action::ToggleNag);
        assert_eq!(nag_of(&effects), Some(Clearable::Set(15)));
        let mut nagging = reminder();
        nagging["extensions"] = json!([{ "nag": 15 }]);
        let mut app = with(vec![task("t1", "Call mum", nagging)]);
        assert_eq!(app.tasks[0].nag, Some(15));
        let effects = act(&mut app, Action::ToggleNag);
        assert_eq!(nag_of(&effects), Some(Clearable::Clear));
    }

    #[test]
    fn a_task_without_a_reminder_is_refused_before_asking() {
        let mut app = with(vec![task("t1", "Call mum", json!({}))]);
        let effects = act(&mut app, Action::ToggleNag);
        assert!(effects.is_empty());
        let banner = app.banner.as_ref().expect("a banner");
        assert_eq!(banner.level, Level::Error);
        assert!(banner.text.contains("no reminder"), "{}", banner.text);
    }
}
