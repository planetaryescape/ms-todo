//! Multi-select: which tasks `x` and `d` act on. The selection holds task
//! IDs of the scope on screen only: switching scope clears it, and an ID
//! whose row goes (deleted, moved out of the view, filtered away) drops
//! out, so a bulk action never reaches a task that isn't shown.

use super::{App, Task};

impl App {
    /// `v`: add the task under the cursor to the selection, or take it out.
    pub(super) fn toggle_select(&mut self) {
        if self.still_loading() {
            return;
        }
        if let Some(id) = self.selected().map(|task| task.id.clone())
            && !self.selection.remove(&id)
        {
            self.selection.insert(id);
        }
    }

    /// `V`: select every task in the view.
    pub(super) fn select_all(&mut self) {
        if self.still_loading() {
            return;
        }
        self.selection = self.tasks.iter().map(|task| task.id.clone()).collect();
    }

    /// Drop the IDs whose rows are gone.
    pub(super) fn prune_selection(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        let shown: std::collections::HashSet<&str> =
            self.tasks.iter().map(|task| task.id.as_str()).collect();
        self.selection.retain(|id| shown.contains(id.as_str()));
    }

    /// The tasks an action takes: the selection, in the order shown, or
    /// else the task under the cursor. None while loading.
    pub(super) fn targets(&self) -> Vec<&Task> {
        if self.loading() {
            return Vec::new();
        }
        if self.selection.is_empty() {
            return self.selected().into_iter().collect();
        }
        self.tasks
            .iter()
            .filter(|task| self.selection.contains(&task.id))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use ms_todo_protocol::{Event, Request, Scope, TaskChange};
    use serde_json::json;

    use crate::action::Action;
    use crate::app::tests::{act, answer_seed, home_tasks, scope_home, seed, seeded, task};
    use crate::app::{Effect, Mode, Msg, Tag, Write};

    fn selected(app: &crate::app::App) -> Vec<&str> {
        let mut ids: Vec<&str> = app.selection.iter().map(String::as_str).collect();
        ids.sort_unstable();
        ids
    }

    fn one_change(effects: &[Effect]) -> (Tag, Vec<String>, TaskChange) {
        match effects {
            [
                Effect {
                    tag,
                    request: Request::ChangeTasks { tasks, change, .. },
                },
            ] => (*tag, tasks.clone(), change.clone()),
            other => unreachable!("not one change: {other:?}"),
        }
    }

    #[test]
    fn v_toggles_capital_v_selects_all_and_escape_clears() {
        let mut app = seeded();
        act(&mut app, Action::ToggleSelect);
        act(&mut app, Action::MoveDown);
        act(&mut app, Action::ToggleSelect);
        assert_eq!(selected(&app), ["t1", "t2"]);
        act(&mut app, Action::ToggleSelect);
        assert_eq!(selected(&app), ["t1"]);
        act(&mut app, Action::SelectAll);
        assert_eq!(selected(&app), ["t1", "t2", "t3", "t4"]);
        // Esc takes the selection first, and the filter only after.
        app.filter = Some("rent".into());
        assert!(act(&mut app, Action::Clear).is_empty());
        assert!(app.selection.is_empty());
        assert_eq!(app.filter.as_deref(), Some("rent"));
    }

    #[test]
    fn bulk_complete_sends_one_change_tasks_with_every_open_id() {
        let mut app = seeded();
        act(&mut app, Action::SelectAll);
        let (tag, tasks, change) = one_change(&act(&mut app, Action::ToggleComplete));
        // In the order shown; Ship blueprint is already completed.
        assert_eq!(
            (tag, tasks, change),
            (
                Tag::Write(Write::Complete),
                vec!["t1".into(), "t2".into(), "t4".into()],
                TaskChange::Complete
            )
        );
        assert!(app.selection.is_empty(), "done with once sent");

        // Only completed tasks selected: reopened together.
        act(&mut app, Action::JumpBottom);
        act(&mut app, Action::ToggleSelect);
        let (tag, tasks, change) = one_change(&act(&mut app, Action::ToggleComplete));
        assert_eq!(
            (tag, tasks, change),
            (
                Tag::Write(Write::Reopen),
                vec!["t3".into()],
                TaskChange::Reopen
            )
        );
    }

    #[test]
    fn bulk_delete_asks_with_the_count_then_sends_one_request() {
        let mut app = seeded();
        for _ in 0..3 {
            act(&mut app, Action::ToggleSelect);
            act(&mut app, Action::MoveDown);
        }
        assert!(act(&mut app, Action::Delete).is_empty());
        assert_eq!(
            app.mode,
            Mode::ConfirmDelete {
                ids: vec!["t1".into(), "t2".into(), "t4".into()],
                what: "3 tasks".into()
            }
        );
        let (tag, tasks, change) = one_change(&act(&mut app, Action::Confirm));
        assert_eq!(
            (tag, tasks.len(), change),
            (Tag::Write(Write::Delete), 3, TaskChange::Delete)
        );
        assert!(app.selection.is_empty());
    }

    #[test]
    fn switching_scope_clears_the_selection() {
        let mut app = seeded();
        act(&mut app, Action::SelectAll);
        act(&mut app, Action::FocusLeft);
        let effects = act(&mut app, Action::MoveUp);
        assert!(app.selection.is_empty());
        // Nothing selected acts on the old scope's rows while loading.
        act(&mut app, Action::FocusRight);
        act(&mut app, Action::ToggleSelect);
        act(&mut app, Action::SelectAll);
        assert!(app.selection.is_empty());
        let tasks = Scope::List { id: "tasks".into() };
        answer_seed(&mut app, &effects[0], seed(tasks, Vec::new()));
        assert!(app.selection.is_empty());
    }

    #[test]
    fn a_selected_task_that_disappears_drops_out() {
        let mut app = seeded();
        act(&mut app, Action::SelectAll);
        // Call Sam was deleted on the phone.
        let effects = app.update(Msg::Event(Event::ResyncNeeded));
        let mut tasks = home_tasks();
        tasks.remove(1);
        answer_seed(&mut app, &effects[0], seed(scope_home(), tasks));
        assert_eq!(selected(&app), ["t1", "t3", "t4"]);

        // And a write that takes a row out of the view drops it too.
        app.shown = Some(Scope::All);
        app.wanted = app.shown.clone();
        let done = task("t4", "Water plants", json!({ "status": "completed" }));
        app.update(Msg::Response {
            tag: Tag::Write(Write::Complete),
            result: Ok(crate::app::tests::applied(
                ms_todo_protocol::TaskAction::Complete,
                vec![done],
            )),
        });
        assert_eq!(selected(&app), ["t1", "t3"]);
    }
}
