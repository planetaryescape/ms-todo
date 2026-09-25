// Ranking is adapted from mxr crates/tui/src/ui/command_palette.rs @ fec4aefce12fc8e2feb93b9652937ceb534e02c0
// (`match_score`: exact, then prefix, then a word's prefix, then
// substring, then the shortcut). Changes: the commands are read from the
// keybinding registry plus a "Go to" for each sidebar row, rather than a
// hand-kept list; a last tier matches the query's letters in order, so
// "gtgr" finds "Go to Groceries"; no categories or recent commands.

//! The command palette (`:`): every action in the keybinding registry and
//! every list and view, even one in a collapsed folder, found by typing
//! part of the name.

use ms_todo_protocol::Scope;

use super::scope::VIEWS;
use super::{App, Effect, LineEditor, Mode, Pane};
use crate::action::Action;
use crate::keybindings;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Run(Action),
    Go(Scope),
    /// Open the theme picker.
    Themes,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub label: String,
    /// The keys that do the same, empty for "Go to".
    pub keys: String,
    pub command: Command,
}

impl App {
    /// Everything the palette offers, best match for `query` first.
    pub fn palette_items(&self, query: &str) -> Vec<Item> {
        let actions = keybindings::commands()
            .into_iter()
            .map(|(keys, label, action)| Item {
                label: label.to_owned(),
                keys,
                command: Command::Run(action),
            });
        let lists = self.lists.iter().map(|list| Scope::List {
            id: list.id.clone(),
        });
        let places = VIEWS.iter().cloned().chain(lists).map(|scope| Item {
            label: format!("Go to {}", self.scope_name(Some(&scope))),
            keys: String::new(),
            command: Command::Go(scope),
        });
        let themes = Item {
            label: "Theme\u{2026}".into(),
            keys: String::new(),
            command: Command::Themes,
        };
        let query = query.trim().to_lowercase();
        rank(
            actions.chain(std::iter::once(themes)).chain(places),
            |item| score(item, &query),
        )
    }

    pub(super) fn open_palette(&mut self) {
        self.mode = Mode::Palette {
            query: LineEditor::single(""),
            index: 0,
        };
    }

    /// Up and Down in the palette, within its matches.
    pub(super) fn palette_step(&mut self, down: bool) {
        let Mode::Palette { query, .. } = &self.mode else {
            return;
        };
        let count = self.palette_items(&query.text()).len();
        if let Mode::Palette { index, .. } = &mut self.mode {
            *index = step(*index, count, down);
        }
    }

    /// Enter in the palette: run the chosen command as if its key had been
    /// pressed in the task list.
    pub(super) fn run_palette(&mut self) -> Vec<Effect> {
        let Mode::Palette { query, index } = std::mem::replace(&mut self.mode, Mode::Normal) else {
            return Vec::new();
        };
        let Some(item) = self.palette_items(&query.text()).into_iter().nth(index) else {
            return Vec::new();
        };
        match item.command {
            Command::Run(action) => {
                if self.focus == Pane::Sidebar {
                    self.focus = Pane::Tasks;
                }
                self.browse(action)
            }
            Command::Themes => {
                self.open_themes();
                Vec::new()
            }
            Command::Go(scope) => {
                self.focus = Pane::Tasks;
                // A list in a collapsed folder: open the folder to show it.
                if let Scope::List { id } = &scope
                    && let Some(folder) = self.folder_of(id).map(str::to_owned)
                {
                    self.collapsed.remove(&folder);
                }
                match self
                    .entries()
                    .iter()
                    .position(|entry| entry.scope().as_ref() == Some(&scope))
                {
                    Some(index) => self.open_entry(index),
                    None => Vec::new(),
                }
            }
        }
    }
}

/// The items that match, best `score` first, ties in their given order.
pub(super) fn rank<T>(
    items: impl Iterator<Item = T>,
    score: impl Fn(&T) -> Option<(u8, usize)>,
) -> Vec<T> {
    let mut ranked: Vec<((u8, usize), usize, T)> = items
        .enumerate()
        .filter_map(|(order, item)| Some((score(&item)?, order, item)))
        .collect();
    ranked.sort_by_key(|(score, order, _)| (*score, *order));
    ranked.into_iter().map(|(_, _, item)| item).collect()
}

/// A picker's choice after Up or Down, within `count` matches.
pub(super) fn step(index: usize, count: usize, down: bool) -> usize {
    if down {
        (index + 1).min(count.saturating_sub(1))
    } else {
        index.saturating_sub(1)
    }
}

/// How well `item` matches `query`, lower first; `None` when it doesn't.
/// The second number orders fuzzy matches by how spread out they are.
fn score(item: &Item, query: &str) -> Option<(u8, usize)> {
    let text = text_score(&item.label, query);
    if matches!(text, Some((tier, _)) if tier <= 3) {
        return text;
    }
    if item.keys.to_lowercase().split('/').any(|key| key == query) {
        return Some((4, 0));
    }
    text
}

/// How well `label` matches `query` (already lowercase), lower first:
/// exact, prefix, a word's prefix, substring, then the query's letters in
/// order, by how spread out they are. `None` when it doesn't match.
pub(super) fn text_score(label: &str, query: &str) -> Option<(u8, usize)> {
    if query.is_empty() {
        return Some((0, 0));
    }
    let label = label.to_lowercase();
    if label == query {
        return Some((0, 0));
    }
    if label.starts_with(query) {
        return Some((1, 0));
    }
    if label.split_whitespace().any(|word| word.starts_with(query)) {
        return Some((2, 0));
    }
    if label.contains(query) {
        return Some((3, 0));
    }
    spread(&label, query).map(|spread| (5, spread))
}

/// How many characters of `label` it takes to find `query`'s letters in
/// order, ignoring its spaces; `None` when they aren't all there.
fn spread(label: &str, query: &str) -> Option<usize> {
    let mut wanted = query.chars().filter(|ch| !ch.is_whitespace()).peekable();
    let mut first = None;
    for (at, ch) in label.chars().enumerate() {
        if wanted.peek() == Some(&ch) {
            wanted.next();
            first.get_or_insert(at);
            if wanted.peek().is_none() {
                return Some(at + 1 - first.unwrap_or(at));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use ms_todo_protocol::Request;

    use super::*;
    use crate::app::Msg;
    use crate::app::tests::{act, seeded};

    fn labels(app: &App, query: &str) -> Vec<String> {
        app.palette_items(query)
            .into_iter()
            .map(|item| item.label)
            .collect()
    }

    #[test]
    fn the_query_finds_actions_and_lists_best_match_first() {
        let app = seeded();
        let everything = labels(&app, "");
        assert!(everything.contains(&"Delete".to_owned()));
        assert!(everything.contains(&"Go to Home".to_owned()));
        assert!(everything.contains(&"Go to Important".to_owned()));
        assert!(!everything.contains(&"Down".to_owned()), "no movement");

        assert_eq!(labels(&app, "del")[0], "Delete");
        assert_eq!(labels(&app, "DIAG")[0], "Diagnostics");
        // A word's start, then anywhere, then the letters in order.
        assert_eq!(labels(&app, "home")[0], "Go to Home");
        assert_eq!(labels(&app, "gthm"), ["Go to Home"]);
        // A key finds its action.
        assert_eq!(labels(&app, "x"), ["Done"]);
        assert!(labels(&app, "zzz").is_empty());
    }

    #[test]
    fn colon_opens_it_typing_filters_and_enter_runs_the_choice() {
        let mut app = seeded();
        act(&mut app, Action::Palette);
        assert_eq!(app.context(), crate::keybindings::Context::Palette);
        for ch in "sync".chars() {
            app.update(Msg::Char(ch));
        }
        let effects = act(&mut app, Action::Submit);
        assert_eq!(effects.len(), 1);
        assert_eq!(effects[0].request, Request::Sync { wait: false });
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn go_to_switches_scope_like_the_sidebar() {
        let mut app = seeded();
        act(&mut app, Action::Palette);
        for ch in "go to all".chars() {
            app.update(Msg::Char(ch));
        }
        let effects = act(&mut app, Action::Submit);
        assert_eq!(
            effects[0].request,
            Request::Seed {
                scope: Some(Scope::All),
                search: None
            }
        );
        assert_eq!(app.wanted, Some(Scope::All));
        assert_eq!(app.sidebar_index, 3);
    }

    #[test]
    fn up_and_down_stay_within_the_matches_and_escape_closes() {
        let mut app = seeded();
        act(&mut app, Action::Palette);
        for ch in "go to".chars() {
            app.update(Msg::Char(ch));
        }
        let matches = app.palette_items("go to").len();
        for _ in 0..matches + 3 {
            act(&mut app, Action::MoveDown);
        }
        assert!(matches!(app.mode, Mode::Palette { index, .. } if index == matches - 1));
        act(&mut app, Action::Backspace);
        assert!(matches!(app.mode, Mode::Palette { index: 0, .. }));
        assert!(act(&mut app, Action::Cancel).is_empty());
        assert_eq!(app.mode, Mode::Normal);
    }
}
