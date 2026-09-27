//! A task's related tasks in the detail pane (`tasks relate`, D-067):
//! Enter on one opens it, in its own list, with the cursor on it.

use ms_todo_protocol::Scope;

use super::{App, Effect, Level, Pane, Task};

impl App {
    /// Enter on a related task: select it, going to its list if it isn't
    /// on screen.
    pub(super) fn open_related(&mut self, task: &Task, at: usize) -> Vec<Effect> {
        let Some(other) = task.related.get(at) else {
            return Vec::new();
        };
        let (Some(id), Some(list_id)) = (other.id.clone(), other.list_id.clone()) else {
            self.show(
                Level::Info,
                "That task isn't here: deleted, or not synced yet (`ms-todo sync`)",
            );
            return Vec::new();
        };
        self.focus = Pane::Tasks;
        if let Some(index) = self.tasks.iter().position(|task| task.id == id) {
            self.task_index = index;
            return Vec::new();
        }
        let scope = Scope::List {
            id: list_id.clone(),
        };
        if let Some(folder) = self.folder_of(&list_id).map(str::to_owned) {
            self.collapsed.remove(&folder);
        }
        let Some(entry) = self
            .entries()
            .iter()
            .position(|entry| entry.scope().as_ref() == Some(&scope))
        else {
            self.show(
                Level::Info,
                "Its list isn't in the sidebar: the context leaves it out (`c` changes it)",
            );
            return Vec::new();
        };
        self.open_after_seed = Some(id.clone());
        let effects = self.open_entry(entry);
        // A list painted from the cache has the task already.
        if let Some(index) = self.tasks.iter().position(|task| task.id == id) {
            self.task_index = index;
        }
        effects
    }
}

#[cfg(test)]
mod tests {
    use ms_todo_protocol::{Request, Scope};
    use serde_json::json;

    use crate::action::Action;
    use crate::app::folders::tests::foldered;
    use crate::app::steps::DetailRow;
    use crate::app::tests::{act, entity};
    use crate::app::{Pane, Task};

    #[test]
    fn enter_on_a_related_task_opens_its_list_with_the_cursor_on_it() {
        let mut app = foldered();
        let mut linked = app.tasks[0].clone();
        linked.related = Task::from_entity(&entity(json!({
            "id": linked.id,
            "related": [
                { "id": "far", "graph_id": "G", "title": "Far away", "list_id": "launch" },
                { "id": null, "graph_id": "H", "title": null, "list_id": null }
            ]
        })))
        .expect("task")
        .related;
        app.tasks[0] = linked;
        app.task_index = 0;
        app.focus = Pane::Detail;
        app.detail_row = DetailRow::Related(1);
        assert!(act(&mut app, Action::EditHere).is_empty(), "not here");
        assert!(app.banner.is_some());

        app.focus = Pane::Detail;
        app.detail_row = DetailRow::Related(0);
        let effects = act(&mut app, Action::EditHere);
        assert!(
            effects.iter().any(|effect| matches!(
                &effect.request,
                Request::Seed { scope: Some(Scope::List { id }), .. } if id == "launch"
            )),
            "{effects:?}"
        );
        assert_eq!(app.focus, Pane::Tasks);
        assert_eq!(app.open_after_seed.as_deref(), Some("far"));
    }
}
