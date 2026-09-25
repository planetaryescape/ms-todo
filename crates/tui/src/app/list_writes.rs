//! "New list…", "Rename list…" and "Delete list…" from the palette
//! (rung 8e): a prompt for the name, or an inline confirmation, then a
//! `ChangeLists` the daemon queues. The sidebar follows with the seed the
//! change's `EntityChanged` brings.

use ms_todo_protocol::{Applied, ListChange, Request, Scope};
use serde_json::Value;

use super::line_editor::LineEditor;
use super::{App, Effect, Level, Mode, Tag};

impl App {
    /// "New list…": ask for its name.
    pub(super) fn start_new_list(&mut self) {
        self.mode = Mode::NamingList {
            list_id: None,
            input: LineEditor::single(""),
        };
    }

    /// "Rename list…": ask for the current list's new name, its name
    /// filled in.
    pub(super) fn start_rename_list(&mut self) {
        let Some(list_id) = self.list_to_move() else {
            self.show(Level::Info, "Choose a list first; a view can't be renamed");
            return;
        };
        let name = self.list_name(&list_id).unwrap_or_default().to_owned();
        self.mode = Mode::NamingList {
            list_id: Some(list_id),
            input: LineEditor::single(&name),
        };
    }

    /// "Delete list…": ask, naming the list; the default list is refused
    /// here, as the daemon would.
    pub(super) fn start_delete_list(&mut self) {
        let Some(list_id) = self.list_to_move() else {
            self.show(Level::Info, "Choose a list first; a view can't be deleted");
            return;
        };
        let Some(list) = self.lists.iter().find(|list| list.id == list_id) else {
            return;
        };
        if list.default {
            self.show(
                Level::Error,
                "Tasks is Microsoft To Do's own list: it can't be deleted",
            );
            return;
        }
        self.mode = Mode::ConfirmDeleteList {
            what: format!("the list \"{}\" and every task in it", list.name),
            list_id,
        };
    }

    /// Enter in the name prompt: make the list, or rename it. An empty
    /// name changes nothing.
    pub(super) fn submit_list_name(&mut self) -> Vec<Effect> {
        let Mode::NamingList { list_id, input } = std::mem::replace(&mut self.mode, Mode::Normal)
        else {
            return Vec::new();
        };
        let name = input.text().trim().to_owned();
        if name.is_empty() {
            return Vec::new();
        }
        let change = match list_id {
            Some(list) => ListChange::RenameList { list, name },
            None => ListChange::CreateList { name, folder: None },
        };
        vec![list_change(change)]
    }

    /// `y` in the confirmation: delete the list.
    pub(super) fn confirm_delete_list(&mut self) -> Vec<Effect> {
        let Mode::ConfirmDeleteList { list_id, .. } =
            std::mem::replace(&mut self.mode, Mode::Normal)
        else {
            return Vec::new();
        };
        vec![list_change(ListChange::DeleteList { list: list_id })]
    }

    /// The daemon's answer to a list's create, rename or delete: say so,
    /// and leave a deleted list for the default one.
    pub(super) fn list_changed(&mut self, applied: &Applied) -> Vec<Effect> {
        let name = applied
            .items
            .first()
            .and_then(|list| list.get("displayName"))
            .and_then(Value::as_str)
            .unwrap_or("the list")
            .to_owned();
        let (text, gone) = match applied.action {
            ms_todo_protocol::TaskAction::CreateList => (format!("Made {name}"), None),
            ms_todo_protocol::TaskAction::RenameList => (format!("Renamed to {name}"), None),
            _ => (
                format!("Deleted {name}"),
                applied
                    .items
                    .first()
                    .and_then(|list| list.get("id"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            ),
        };
        self.show(Level::Info, &text);
        // The change's `EntityChanged` reseeds the sidebar; a deleted list
        // on screen needs a seed of the default list instead.
        if let Some(gone) = gone
            && self.wanted == Some(Scope::List { id: gone.clone() })
        {
            self.wanted = None;
            self.shown = None;
            return self.reseed();
        }
        Vec::new()
    }
}

fn list_change(change: ListChange) -> Effect {
    Effect {
        tag: Tag::Lists,
        request: Request::ChangeLists {
            change,
            dry_run: false,
            op_id: None,
            idempotency_key: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::action::Action;
    use crate::app::Msg;
    use crate::app::tests::{entity, seeded};

    #[test]
    fn a_list_is_made_renamed_and_deleted_from_the_palette() {
        let mut app = seeded();
        app.update(Msg::Action(Action::NewList));
        for ch in "Garden".chars() {
            app.update(Msg::Char(ch));
        }
        let effects = app.update(Msg::Action(Action::Submit));
        assert_eq!(
            effects[0].request,
            Request::ChangeLists {
                change: ListChange::CreateList {
                    name: "Garden".into(),
                    folder: None,
                },
                dry_run: false,
                op_id: None,
                idempotency_key: None,
            }
        );

        app.wanted = Some(Scope::List { id: "home".into() });
        app.shown = app.wanted.clone();
        app.sidebar_index = usize::MAX;
        app.update(Msg::Action(Action::RenameList));
        assert!(matches!(&app.mode, Mode::NamingList { input, .. } if input.text() == "Home"));
        app.update(Msg::Action(Action::Cancel));

        app.update(Msg::Action(Action::DeleteList));
        assert!(matches!(&app.mode, Mode::ConfirmDeleteList { what, .. } if what.contains("Home")));
        let effects = app.update(Msg::Action(Action::Confirm));
        assert!(matches!(
            &effects[0].request,
            Request::ChangeLists { change: ListChange::DeleteList { list }, .. } if list == "home"
        ));
        app.update(Msg::Response {
            tag: Tag::Lists,
            result: Ok(ms_todo_protocol::ResponseData::Applied(Applied {
                op_id: "op".into(),
                action: ms_todo_protocol::TaskAction::DeleteList,
                items: vec![entity(json!({ "id": "home", "displayName": "Home" }))],
                list_ids: vec!["home".into()],
                rolled: Vec::new(),
                undoes: None,
                refused: Vec::new(),
            })),
        });
        assert_eq!(app.wanted, None, "off the deleted list");
    }
}
