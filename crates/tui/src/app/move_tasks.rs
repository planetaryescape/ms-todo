//! "Move to list…" (`m`, docs/blueprint/08-tui.md): the selection, or the
//! task under the cursor, moved to a list picked by typing part of its
//! name or its folder's. The tasks are the ones on screen when `m` is
//! pressed, never anything outside the current scope; one `ChangeTasks`
//! moves them all, and one `u` moves them back.

use ms_todo_protocol::{Applied, ListChange, TaskChange};

use super::palette::{rank, step, text_score};
use super::{App, Effect, Level, LineEditor, Mode, Write, change};

/// A list tasks can move to, as the picker shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoveTarget {
    pub id: String,
    pub name: String,
    pub folder: Option<String>,
}

impl App {
    /// The lists the tasks `ids` can move to, best match for `query`
    /// first, else in the sidebar's order: every list but one holding
    /// every task already.
    pub fn move_targets(&self, ids: &[String], query: &str) -> Vec<MoveTarget> {
        let from: Vec<&str> = self
            .tasks
            .iter()
            .filter(|task| ids.contains(&task.id))
            .map(|task| task.list_id.as_str())
            .collect();
        let query = query.trim().to_lowercase();
        let lists = self
            .lists
            .iter()
            .filter(|list| from.is_empty() || from.iter().any(|from| *from != list.id));
        let label = |target: &MoveTarget| match &target.folder {
            Some(folder) => format!("{} {folder}", target.name),
            None => target.name.clone(),
        };
        rank(
            lists.map(|list| MoveTarget {
                id: list.id.clone(),
                name: list.name.clone(),
                folder: list.folder.clone(),
            }),
            |target| text_score(&label(target), &query),
        )
    }

    /// The lists the picker offers: [`App::move_targets`], but never the
    /// list being merged.
    pub fn picker_targets(
        &self,
        ids: &[String],
        merging: Option<&str>,
        query: &str,
    ) -> Vec<MoveTarget> {
        let mut targets = self.move_targets(ids, query);
        targets.retain(|target| Some(target.id.as_str()) != merging);
        targets
    }

    /// "Merge list into…": the picker, for every open task of the current
    /// list.
    pub(super) fn start_merge_list(&mut self) {
        let Some(list_id) = self.list_to_move() else {
            self.show(Level::Info, "Choose a list first; a view can't be merged");
            return;
        };
        let name = self.list_name(&list_id).unwrap_or("the list").to_owned();
        self.mode = Mode::MovingTasks {
            ids: Vec::new(),
            what: format!("every open task of {name}"),
            query: LineEditor::single(""),
            index: 0,
            merging: Some(list_id),
        };
    }

    /// `m`: open the picker for the selection, or the task under the
    /// cursor.
    pub(super) fn start_move_tasks(&mut self) {
        let targets = self.targets();
        let what = match targets.as_slice() {
            [] => return,
            [task] => format!("\"{}\"", task.title),
            many => format!("{} tasks", many.len()),
        };
        let ids = targets.iter().map(|task| task.id.clone()).collect();
        self.mode = Mode::MovingTasks {
            ids,
            what,
            query: LineEditor::single(""),
            index: 0,
            merging: None,
        };
    }

    /// Up and Down in the picker, within its matches.
    pub(super) fn move_step(&mut self, down: bool) {
        let Mode::MovingTasks {
            ids,
            query,
            merging,
            ..
        } = &self.mode
        else {
            return;
        };
        let count = self
            .picker_targets(ids, merging.as_deref(), &query.text())
            .len();
        if let Mode::MovingTasks { index, .. } = &mut self.mode {
            *index = step(*index, count, down);
        }
    }

    /// Enter in the picker: move the tasks to the chosen list.
    pub(super) fn submit_move_tasks(&mut self) -> Vec<Effect> {
        let Mode::MovingTasks {
            ids,
            query,
            index,
            merging,
            ..
        } = std::mem::replace(&mut self.mode, Mode::Normal)
        else {
            return Vec::new();
        };
        let Some(target) = self
            .picker_targets(&ids, merging.as_deref(), &query.text())
            .into_iter()
            .nth(index)
        else {
            return Vec::new();
        };
        if let Some(from) = merging {
            // The source stays: deleting it is the CLI's `--delete-source`,
            // not a picker's side effect.
            let change = ListChange::MergeList {
                from,
                into: target.id,
                include_completed: false,
                delete_source: false,
            };
            return vec![super::list_writes::list_change(change)];
        }
        self.selection.clear();
        vec![change(Write::Move, ids, TaskChange::Move { to: target.id })]
    }

    /// A move's answer: say where the tasks went.
    pub(super) fn moved(&mut self, applied: &Applied) {
        let list = applied
            .list_ids
            .first()
            .and_then(|id| self.list_name(id))
            .unwrap_or("the list")
            .to_owned();
        let text = match applied.items.len() {
            0 => "Nothing to move".to_owned(),
            1 => format!("Moving to {list}; u moves it back"),
            count => format!("Moving {count} tasks to {list}; u moves them back"),
        };
        self.show(Level::Info, &text);
    }
}

#[cfg(test)]
mod tests {
    use ms_todo_protocol::{Request, ResponseData, Scope};
    use serde_json::json;

    use super::*;
    use crate::action::Action;
    use crate::app::folders::tests::foldered;
    use crate::app::tests::{act, entity};
    use crate::app::{Msg, Tag};
    use crate::keybindings::Context;

    fn names(targets: &[MoveTarget]) -> Vec<&str> {
        targets.iter().map(|target| target.name.as_str()).collect()
    }

    fn moves(effects: &[Effect]) -> (Vec<String>, String) {
        match effects {
            [
                Effect {
                    tag: Tag::Write(Write::Move),
                    request:
                        Request::ChangeTasks {
                            tasks,
                            change: TaskChange::Move { to },
                            ..
                        },
                },
            ] => (tasks.clone(), to.clone()),
            other => unreachable!("not one move: {other:?}"),
        }
    }

    #[test]
    fn m_opens_a_picker_of_the_other_lists_with_their_folders() {
        let mut app = foldered();
        act(&mut app, Action::MoveTasks);
        assert_eq!(app.context(), Context::MoveTo);
        let Mode::MovingTasks { ids, what, .. } = &app.mode else {
            unreachable!("{:?}", app.mode);
        };
        assert_eq!(ids.len(), 1);
        assert!(what.starts_with('"'), "{what}");
        // Home holds the task, so it isn't offered.
        let all = app.move_targets(ids, "");
        assert_eq!(names(&all), ["Finances", "Launch", "Tasks"]);
        assert_eq!(all[0].folder.as_deref(), Some("Areas"));
        // A folder's name finds its lists; letters in order find a list.
        assert_eq!(names(&app.move_targets(ids, "projects")), ["Launch"]);
        assert_eq!(names(&app.move_targets(ids, "fnc")), ["Finances"]);
    }

    #[test]
    fn typing_narrows_and_enter_moves_only_the_selection_on_screen() {
        let mut app = foldered();
        act(&mut app, Action::SelectAll);
        let shown: Vec<String> = app.tasks.iter().map(|task| task.id.clone()).collect();
        assert!(shown.len() > 1);
        act(&mut app, Action::MoveTasks);
        for ch in "launch".chars() {
            app.update(Msg::Char(ch));
        }
        let effects = act(&mut app, Action::Submit);
        let (mut tasks, to) = moves(&effects);
        tasks.sort();
        let mut expected = shown.clone();
        expected.sort();
        assert_eq!(tasks, expected, "exactly the tasks on screen");
        assert_eq!(to, "launch");
        assert!(app.selection.is_empty());
        assert_eq!(app.mode, Mode::Normal);

        // The answer takes them out of Home at once, and says so.
        let items: Vec<_> = shown
            .iter()
            .map(|id| {
                entity(
                    json!({ "id": id, "title": "t", "status": "notStarted", "list_id": "launch" }),
                )
            })
            .collect();
        app.update(Msg::Response {
            tag: Tag::Write(Write::Move),
            result: Ok(ResponseData::Applied(Applied {
                op_id: "op-1".into(),
                action: ms_todo_protocol::TaskAction::Move,
                items,
                list_ids: vec!["launch".into(); shown.len()],
                rolled: Vec::new(),
                undoes: None,
                refused: Vec::new(),
            })),
        });
        assert!(app.tasks.is_empty(), "{:?}", app.tasks);
        assert!(
            app.banner
                .as_ref()
                .is_some_and(|banner| banner.text.contains("to Launch")),
            "{:?}",
            app.banner
        );
    }

    #[test]
    fn merge_list_into_picks_any_other_list_and_merges_the_current_one() {
        let mut app = foldered();
        let labels: Vec<String> = app
            .palette_items("merge")
            .into_iter()
            .map(|item| item.label)
            .collect();
        assert_eq!(
            labels.first().map(String::as_str),
            Some("Merge list into\u{2026}")
        );
        act(&mut app, Action::MergeList);
        let Mode::MovingTasks {
            ids, what, merging, ..
        } = &app.mode
        else {
            unreachable!("{:?}", app.mode);
        };
        assert!(ids.is_empty());
        assert_eq!(merging.as_deref(), Some("home"));
        assert!(what.contains("Home"), "{what}");
        let offered = app.picker_targets(ids, merging.as_deref(), "");
        assert_eq!(names(&offered), ["Finances", "Launch", "Tasks"]);
        for ch in "launch".chars() {
            app.update(Msg::Char(ch));
        }
        let effects = act(&mut app, Action::Submit);
        assert_eq!(
            effects[0].request,
            Request::ChangeLists {
                change: ListChange::MergeList {
                    from: "home".into(),
                    into: "launch".into(),
                    include_completed: false,
                    delete_source: false,
                },
                dry_run: false,
                op_id: None,
                idempotency_key: None,
            }
        );
        assert_eq!(effects[0].tag, crate::app::Tag::Lists);
    }

    #[test]
    fn down_and_up_stay_in_the_matches_and_nothing_moves_while_loading() {
        let mut app = foldered();
        act(&mut app, Action::MoveTasks);
        for _ in 0..10 {
            act(&mut app, Action::MoveDown);
        }
        assert!(matches!(app.mode, Mode::MovingTasks { index: 2, .. }));
        act(&mut app, Action::MoveUp);
        let effects = act(&mut app, Action::Submit);
        assert_eq!(moves(&effects).1, "launch");

        // A scope still loading has no tasks to take.
        app.wanted = Some(Scope::All);
        act(&mut app, Action::MoveTasks);
        assert_eq!(app.mode, Mode::Normal);
        // Esc closes the picker, sending nothing.
        app.wanted = app.shown.clone();
        act(&mut app, Action::MoveTasks);
        assert!(act(&mut app, Action::Cancel).is_empty());
        assert_eq!(app.mode, Mode::Normal);
    }
}
