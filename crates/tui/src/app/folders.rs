//! Folders in the sidebar (docs/blueprint/08-tui.md#layout): collapsing
//! and expanding a folder, keeping the sidebar's cursor on the same row as
//! rows come and go, and "Move list to folder…".

use ms_todo_protocol::{Entity, ListChange, Request, Scope};
use serde_json::Value;

use super::line_editor::LineEditor;
use super::scope::{Entry, folder_names};
use super::{App, Effect, Level, Mode, Pane, Tag};

/// The most folder names the move prompt suggests at once.
pub const MAX_SUGGESTIONS: usize = 5;

impl App {
    /// Enter or Space in the sidebar: a folder's heading collapses or
    /// expands; any other row opens, moving to the task list.
    pub(super) fn open_row(&mut self) -> Vec<Effect> {
        if self.focus != Pane::Sidebar {
            return Vec::new();
        }
        match self.entries().get(self.sidebar_index) {
            Some(Entry::Folder { name, .. }) => {
                let name = name.clone();
                self.toggle_folder(&name);
            }
            Some(_) => self.focus = Pane::Tasks,
            None => {}
        }
        Vec::new()
    }

    /// Collapse the folder `name`, or expand it, for the rest of the
    /// session. The cursor stays on its heading.
    pub(super) fn toggle_folder(&mut self, name: &str) {
        let row = self.entries().get(self.sidebar_index).cloned();
        if !self.collapsed.remove(name) {
            self.collapsed.insert(name.to_owned());
        }
        self.place_sidebar_cursor(row);
    }

    /// Put the sidebar's cursor back on `row` after the rows changed, else
    /// on the scope shown, else on the heading of the collapsed folder that
    /// hides it; else where it was, within the rows.
    pub(super) fn place_sidebar_cursor(&mut self, row: Option<Entry>) {
        let entries = self.entries();
        let wanted = self.wanted.clone();
        let found = row
            .and_then(|row| entries.iter().position(|entry| entry.same(&row)))
            .or_else(|| {
                let wanted = wanted.as_ref()?;
                entries
                    .iter()
                    .position(|entry| entry.scope().as_ref() == Some(wanted))
            })
            .or_else(|| {
                let Some(Scope::List { id }) = &wanted else {
                    return None;
                };
                let folder = self.folder_of(id)?;
                entries.iter().position(
                    |entry| matches!(entry, Entry::Folder { name, .. } if *name == folder),
                )
            });
        self.sidebar_index = found
            .unwrap_or(self.sidebar_index)
            .min(entries.len().saturating_sub(1));
    }

    /// The list "Move list to folder…" (and a rename or delete) acts on:
    /// the one under the sidebar's cursor, else the one shown.
    pub(super) fn list_to_move(&self) -> Option<String> {
        match self.entries().get(self.sidebar_index) {
            Some(Entry::List { id, .. }) => Some(id.clone()),
            _ => match &self.shown {
                Some(Scope::List { id }) => Some(id.clone()),
                _ => None,
            },
        }
    }

    /// Open the prompt for the folder to move the current list into, with
    /// its folder filled in.
    pub(super) fn start_move(&mut self) {
        let Some(list_id) = self.list_to_move() else {
            self.show(Level::Info, "Choose a list first; a view isn't in a folder");
            return;
        };
        let folder = self.folder_of(&list_id).unwrap_or_default().to_owned();
        self.mode = Mode::MovingList {
            list_id,
            input: LineEditor::single(&folder),
        };
    }

    /// Enter in the prompt: move the list to the folder typed; empty takes
    /// it out of its folder.
    pub(super) fn submit_move(&mut self) -> Vec<Effect> {
        let Mode::MovingList { list_id, input } = std::mem::replace(&mut self.mode, Mode::Normal)
        else {
            return Vec::new();
        };
        let typed = input.text();
        let folder = Some(typed.trim())
            .filter(|name| !name.is_empty())
            .map(str::to_owned);
        vec![Effect {
            tag: Tag::Folders,
            request: Request::ChangeLists {
                change: ListChange::MoveList {
                    lists: vec![list_id],
                    folder,
                },
                dry_run: false,
                op_id: None,
                idempotency_key: None,
            },
        }]
    }

    /// Tab in the prompt: the first suggestion replaces what's typed.
    pub(super) fn complete_folder(&mut self) {
        let Mode::MovingList { input, .. } = &self.mode else {
            return;
        };
        let Some(first) = self.folder_suggestions(&input.text()).into_iter().next() else {
            return;
        };
        if let Mode::MovingList { input, .. } = &mut self.mode {
            *input = LineEditor::single(&first);
        }
    }

    /// The folder the list `id` is in, if any.
    pub fn folder_of(&self, id: &str) -> Option<&str> {
        self.lists
            .iter()
            .find(|list| list.id == id)
            .and_then(|list| list.folder.as_deref())
    }

    /// The folders whose names start with `typed`, ignoring case, in order;
    /// every folder when nothing is typed.
    pub fn folder_suggestions(&self, typed: &str) -> Vec<String> {
        let typed = typed.trim().to_lowercase();
        folder_names(&self.lists)
            .into_iter()
            .filter(|name| name.to_lowercase().starts_with(&typed))
            .take(MAX_SUGGESTIONS)
            .map(str::to_owned)
            .collect()
    }

    /// Draw a folder change's `Applied` answer at once: each list shows in
    /// its new folder before the seed that follows brings the order.
    pub(super) fn apply_list_write(&mut self, items: &[Entity]) {
        let row = self.entries().get(self.sidebar_index).cloned();
        for item in items {
            let Some(id) = item.get("id").and_then(Value::as_str) else {
                continue;
            };
            let folder = item
                .get("folder")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let Some(at) = self.lists.iter().position(|list| list.id == id) else {
                continue;
            };
            if self.lists[at].folder == folder {
                continue;
            }
            // Last in its new folder, as the daemon will have it.
            let mut list = self.lists.remove(at);
            list.folder = folder;
            let to = match &list.folder {
                Some(name) => self
                    .lists
                    .iter()
                    .rposition(|other| other.folder.as_ref() == Some(name))
                    .map(|last| last + 1)
                    .or_else(|| self.lists.iter().position(|other| other.folder.is_none())),
                None => None,
            }
            .unwrap_or(self.lists.len());
            self.lists.insert(to, list);
        }
        if let [item] = items {
            let name = item
                .get("displayName")
                .and_then(Value::as_str)
                .unwrap_or("The list");
            let text = match item.get("folder").and_then(Value::as_str) {
                Some(folder) => format!("Moved {name} to {folder}"),
                None => format!("Took {name} out of its folder"),
            };
            self.show(Level::Info, &text);
        }
        self.place_sidebar_cursor(row);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use ms_todo_protocol::{Applied, ResponseData, Seed, TaskAction};
    use serde_json::json;

    use super::*;
    use crate::action::Action;
    use crate::app::Msg;
    use crate::app::tests::{act, answer_seed, entity, home_tasks, scope_home, seed, seeded};
    use crate::keybindings::Context;

    /// Home and Finances in "Areas", Launch in "Projects", Tasks in none,
    /// as the daemon orders them; Home shown.
    pub(crate) fn foldered_seed() -> Seed {
        let mut seed = seed(scope_home(), home_tasks());
        seed.lists = vec![
            entity(json!({ "id": "home", "displayName": "Home", "folder": "Areas" })),
            entity(json!({ "id": "fin", "displayName": "Finances", "folder": "Areas" })),
            entity(json!({ "id": "launch", "displayName": "Launch", "folder": "Projects" })),
            entity(json!({ "id": "tasks", "displayName": "Tasks", "folder": null })),
        ];
        seed.counts.lists.insert("fin".into(), 2);
        seed.counts.lists.insert("launch".into(), 4);
        seed
    }

    pub(crate) fn foldered() -> App {
        let mut app = seeded();
        let effects = act(&mut app, Action::Sync);
        assert_eq!(effects.len(), 1);
        let reseed = app.update(Msg::Event(ms_todo_protocol::Event::ResyncNeeded));
        answer_seed(&mut app, &reseed[0], foldered_seed());
        app
    }

    fn rows(app: &App) -> Vec<String> {
        app.entries()
            .iter()
            .map(|entry| match entry {
                Entry::View(scope) => format!("{scope:?}"),
                Entry::Folder {
                    name,
                    collapsed,
                    count,
                } => format!("[{}{name} {count}]", if *collapsed { "+" } else { "-" }),
                Entry::List {
                    name, in_folder, ..
                } => format!("{}{name}", if *in_folder { "  " } else { "" }),
            })
            .collect()
    }

    fn cursor(app: &App) -> Option<Entry> {
        app.entries().get(app.sidebar_index).cloned()
    }

    #[test]
    fn folders_group_their_lists_with_a_total_before_the_lists_in_none() {
        let app = foldered();
        assert_eq!(
            rows(&app)[6..],
            [
                "[-Areas 5]",
                "  Home",
                "  Finances",
                "[-Projects 4]",
                "  Launch",
                "Tasks"
            ]
        );
        // Home is still the row shown.
        assert!(matches!(cursor(&app), Some(Entry::List { id, .. }) if id == "home"));
    }

    #[test]
    fn enter_on_a_folder_collapses_and_expands_it_and_the_cursor_stays() {
        let mut app = foldered();
        app.focus = Pane::Sidebar;
        // Up from Home is the Areas heading: nothing is seeded for it.
        assert!(act(&mut app, Action::MoveUp).is_empty());
        assert!(matches!(cursor(&app), Some(Entry::Folder { name, .. }) if name == "Areas"));
        assert_eq!(app.wanted, Some(scope_home()), "the tasks shown stay");

        assert!(act(&mut app, Action::Open).is_empty());
        assert_eq!(
            rows(&app)[6..],
            ["[+Areas 5]", "[-Projects 4]", "  Launch", "Tasks"]
        );
        assert!(matches!(cursor(&app), Some(Entry::Folder { name, .. }) if name == "Areas"));
        assert_eq!(app.focus, Pane::Sidebar);

        // Remembered across seeds, with the cursor on its heading.
        let reseed = app.update(Msg::Event(ms_todo_protocol::Event::ResyncNeeded));
        answer_seed(&mut app, &reseed[0], foldered_seed());
        assert!(app.collapsed.contains("Areas"));
        assert!(matches!(cursor(&app), Some(Entry::Folder { name, .. }) if name == "Areas"));

        act(&mut app, Action::Open);
        assert_eq!(rows(&app).len(), 12);
        // Down onto a list opens it.
        let effects = act(&mut app, Action::MoveDown);
        assert_eq!(effects.len(), 1);
        assert_eq!(app.wanted, Some(scope_home()));
    }

    #[test]
    fn a_seed_keeps_the_cursor_on_the_shown_list_by_id_across_a_collapse() {
        let mut app = foldered();
        app.focus = Pane::Sidebar;
        act(&mut app, Action::MoveUp);
        act(&mut app, Action::Open);
        let reseed = app.update(Msg::Event(ms_todo_protocol::Event::ResyncNeeded));
        answer_seed(&mut app, &reseed[0], foldered_seed());
        // Home, shown, is hidden: its folder's heading stands for it.
        assert_eq!(app.wanted, Some(scope_home()));
        assert!(matches!(cursor(&app), Some(Entry::Folder { name, .. }) if name == "Areas"));

        act(&mut app, Action::Open);
        let opened = act(&mut app, Action::MoveDown);
        assert!(matches!(cursor(&app), Some(Entry::List { id, .. }) if id == "home"));
        // The daemon has reordered the lists: the cursor follows Home's ID.
        let mut moved = foldered_seed();
        moved.lists.swap(0, 1);
        answer_seed(&mut app, &opened[0], moved);
        assert_eq!(rows(&app)[7..9], ["  Finances", "  Home"]);
        assert!(matches!(cursor(&app), Some(Entry::List { id, .. }) if id == "home"));
    }

    #[test]
    fn the_palette_moves_the_list_to_a_folder_suggesting_existing_ones() {
        let mut app = foldered();
        act(&mut app, Action::Palette);
        for ch in "move list".chars() {
            app.update(Msg::Char(ch));
        }
        assert!(act(&mut app, Action::Submit).is_empty());
        assert_eq!(app.context(), Context::Folder);
        // It starts with the list's own folder.
        assert_eq!(
            app.mode,
            Mode::MovingList {
                list_id: "home".into(),
                input: LineEditor::single("Areas"),
            }
        );
        assert_eq!(app.folder_suggestions(""), ["Areas", "Projects"]);

        app.update(Msg::Action(Action::Cancel));
        act(&mut app, Action::MoveToFolder);
        for _ in 0.."Areas".len() {
            act(&mut app, Action::Backspace);
        }
        app.update(Msg::Char('p'));
        assert_eq!(app.folder_suggestions("p"), ["Projects"]);
        act(&mut app, Action::Complete);
        let effects = act(&mut app, Action::Submit);
        assert_eq!(
            effects,
            [Effect {
                tag: Tag::Folders,
                request: Request::ChangeLists {
                    change: ListChange::MoveList {
                        lists: vec!["home".into()],
                        folder: Some("Projects".into()),
                    },
                    dry_run: false,
                    op_id: None,
                    idempotency_key: None,
                },
            }]
        );
        assert_eq!(app.mode, Mode::Normal);

        // The answer moves it at once, before the seed.
        app.update(Msg::Response {
            tag: Tag::Folders,
            result: Ok(ResponseData::Applied(Applied {
                op_id: "op-1".into(),
                action: TaskAction::MoveList,
                items: vec![entity(
                    json!({ "id": "home", "displayName": "Home", "folder": "Projects" }),
                )],
                list_ids: vec!["home".into()],
                rolled: Vec::new(),
                undoes: None,
                refused: Vec::new(),
            })),
        });
        assert_eq!(
            rows(&app)[6..],
            [
                "[-Areas 2]",
                "  Finances",
                "[-Projects 7]",
                "  Launch",
                "  Home",
                "Tasks"
            ]
        );
        assert!(matches!(cursor(&app), Some(Entry::List { id, .. }) if id == "home"));
        assert_eq!(
            app.banner.as_ref().map(|b| b.text.as_str()),
            Some("Moved Home to Projects")
        );
    }

    #[test]
    fn ctrl_u_then_enter_takes_the_list_out_of_its_folder() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = foldered();
        act(&mut app, Action::MoveToFolder);
        app.update(Msg::Key(KeyEvent::new(
            KeyCode::Char('u'),
            KeyModifiers::CONTROL,
        )));
        assert_eq!(
            app.mode,
            Mode::MovingList {
                list_id: "home".into(),
                input: LineEditor::single(""),
            }
        );
        let effects = act(&mut app, Action::Submit);
        assert_eq!(
            effects[0].request,
            Request::ChangeLists {
                change: ListChange::MoveList {
                    lists: vec!["home".into()],
                    folder: None,
                },
                dry_run: false,
                op_id: None,
                idempotency_key: None,
            }
        );
    }

    #[test]
    fn an_empty_folder_name_takes_the_list_out_and_a_view_has_nothing_to_move() {
        let mut app = foldered();
        act(&mut app, Action::MoveToFolder);
        for _ in 0.."Areas".len() {
            act(&mut app, Action::Backspace);
        }
        let effects = act(&mut app, Action::Submit);
        assert!(matches!(
            &effects[0].request,
            Request::ChangeLists {
                change: ListChange::MoveList { folder: None, .. },
                ..
            }
        ));

        app.focus = Pane::Sidebar;
        app.sidebar_index = 0;
        app.shown = Some(Scope::Important);
        act(&mut app, Action::MoveToFolder);
        assert_eq!(app.mode, Mode::Normal);
        assert!(app.banner.is_some());
    }
}
