//! Reordering the sidebar by keys (D-066): `K` and `J` (or Alt-Up and
//! Alt-Down) move the list under the cursor up or down within its folder,
//! or a folder among the folders, with the same `ChangeLists` as `lists
//! order` and `folders order`. The sidebar moves at once; the seed the
//! change's `EntityChanged` brings confirms it.

use ms_todo_protocol::{Anchor, ListChange, Request};

use super::scope::{Entry, folder_names};
use super::{App, Effect, Level, Tag};

impl App {
    /// Move the sidebar row under the cursor one place up or down.
    pub(super) fn reorder(&mut self, down: bool) -> Vec<Effect> {
        let row = self.entries().get(self.sidebar_index).cloned();
        let change = match &row {
            Some(Entry::List { id, .. }) => self.reorder_list(id, down),
            Some(Entry::Folder { name, .. }) => self.reorder_folder(name, down),
            _ => {
                self.show(
                    Level::Info,
                    "Put the sidebar's cursor on a list or a folder to move it",
                );
                return Vec::new();
            }
        };
        let Some(change) = change else {
            self.show(
                Level::Info,
                if down {
                    "Already last; nothing to move past"
                } else {
                    "Already first; nothing to move past"
                },
            );
            return Vec::new();
        };
        self.place_sidebar_cursor(row);
        vec![Effect {
            tag: Tag::Order,
            request: Request::ChangeLists {
                change,
                dry_run: false,
                op_id: None,
                idempotency_key: None,
            },
        }]
    }

    /// Swap the list `id` with its neighbour in the same folder (or in
    /// none), here and in the change to send.
    fn reorder_list(&mut self, id: &str, down: bool) -> Option<ListChange> {
        let at = self.lists.iter().position(|list| list.id == id)?;
        let folder = self.lists[at].folder.clone();
        let mut siblings = self
            .lists
            .iter()
            .enumerate()
            .filter(|(_, list)| list.folder == folder)
            .map(|(index, _)| index);
        let next = if down {
            siblings.find(|index| *index > at)
        } else {
            siblings.rfind(|index| *index < at)
        }?;
        let anchor = self.lists[next].id.clone();
        self.lists.swap(at, next);
        Some(ListChange::OrderList {
            list: id.to_owned(),
            anchor: if down {
                Anchor::After(anchor)
            } else {
                Anchor::Before(anchor)
            },
        })
    }

    /// Move the folder `name` past its neighbour: its lists go before (or
    /// after) that folder's, which is the order the sidebar reads.
    fn reorder_folder(&mut self, name: &str, down: bool) -> Option<ListChange> {
        let names = folder_names(&self.lists);
        let at = names.iter().position(|folder| *folder == name)?;
        let other = if down {
            names.get(at + 1)
        } else {
            at.checked_sub(1).and_then(|before| names.get(before))
        }?
        .to_string();
        let (first, second) = if down {
            (other.as_str(), name)
        } else {
            (name, other.as_str())
        };
        // Stable: the two folders' lists, `first`'s before `second`'s.
        let (moved, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.lists)
            .into_iter()
            .partition(|list| list.folder.as_deref() == Some(first));
        let mut lists = Vec::with_capacity(moved.len() + rest.len());
        let mut moved = Some(moved);
        for list in rest {
            if list.folder.as_deref() == Some(second)
                && let Some(moved) = moved.take()
            {
                lists.extend(moved);
            }
            lists.push(list);
        }
        lists.extend(moved.into_iter().flatten());
        self.lists = lists;
        Some(ListChange::OrderFolder {
            folder: name.to_owned(),
            anchor: if down {
                Anchor::After(other)
            } else {
                Anchor::Before(other)
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use ms_todo_protocol::ErrorPayload;

    use super::*;
    use crate::action::Action;
    use crate::app::folders::tests::foldered;
    use crate::app::tests::act;
    use crate::app::{Msg, Pane};

    fn order(effects: &[Effect]) -> ListChange {
        match effects {
            [
                Effect {
                    tag: Tag::Order,
                    request: Request::ChangeLists { change, .. },
                },
            ] => change.clone(),
            other => unreachable!("not one order: {other:?}"),
        }
    }

    fn names(app: &App) -> Vec<String> {
        app.lists.iter().map(|list| list.name.clone()).collect()
    }

    fn cursor_on(app: &mut App, wanted: &Entry) {
        app.focus = Pane::Sidebar;
        app.sidebar_index = app
            .entries()
            .iter()
            .position(|entry| entry.same(wanted))
            .expect("row");
    }

    fn list(id: &str) -> Entry {
        Entry::List {
            id: id.into(),
            name: String::new(),
            in_folder: false,
        }
    }

    fn folder(name: &str) -> Entry {
        Entry::Folder {
            name: name.into(),
            collapsed: false,
            count: 0,
        }
    }

    #[test]
    fn a_list_moves_within_its_folder_and_the_cursor_follows() {
        let mut app = foldered();
        cursor_on(&mut app, &list("fin"));
        assert_eq!(
            order(&act(&mut app, Action::ReorderUp)),
            ListChange::OrderList {
                list: "fin".into(),
                anchor: Anchor::Before("home".into()),
            }
        );
        assert_eq!(names(&app), ["Finances", "Home", "Launch", "Tasks"]);
        assert!(app.entries()[app.sidebar_index].same(&list("fin")));
        // First in its folder: it doesn't leave it.
        assert!(act(&mut app, Action::ReorderUp).is_empty());
        assert_eq!(
            app.banner.as_ref().map(|banner| banner.text.as_str()),
            Some("Already first; nothing to move past")
        );
        assert_eq!(
            order(&act(&mut app, Action::ReorderDown)),
            ListChange::OrderList {
                list: "fin".into(),
                anchor: Anchor::After("home".into()),
            }
        );
        assert_eq!(names(&app), ["Home", "Finances", "Launch", "Tasks"]);
    }

    #[test]
    fn a_folder_moves_past_its_neighbour_with_its_lists() {
        let mut app = foldered();
        cursor_on(&mut app, &folder("Projects"));
        assert_eq!(
            order(&act(&mut app, Action::ReorderUp)),
            ListChange::OrderFolder {
                folder: "Projects".into(),
                anchor: Anchor::Before("Areas".into()),
            }
        );
        assert_eq!(names(&app), ["Launch", "Home", "Finances", "Tasks"]);
        assert!(app.entries()[app.sidebar_index].same(&folder("Projects")));
        cursor_on(&mut app, &folder("Projects"));
        assert_eq!(
            order(&act(&mut app, Action::ReorderDown)),
            ListChange::OrderFolder {
                folder: "Projects".into(),
                anchor: Anchor::After("Areas".into()),
            }
        );
        assert_eq!(names(&app), ["Home", "Finances", "Launch", "Tasks"]);
    }

    #[test]
    fn a_view_isnt_moved_and_a_refusal_reads_the_order_again() {
        let mut app = foldered();
        app.focus = Pane::Sidebar;
        app.sidebar_index = 0;
        assert!(act(&mut app, Action::ReorderDown).is_empty());
        assert!(app.banner.is_some());
        cursor_on(&mut app, &list("fin"));
        act(&mut app, Action::ReorderUp);
        let effects = app.update(Msg::Response {
            tag: Tag::Order,
            result: Err(ErrorPayload {
                kind: "invalid_input".into(),
                message: "no".into(),
                ..ErrorPayload::default()
            }),
        });
        assert!(matches!(effects[0].request, Request::Seed { .. }));
    }
}
