//! Moving the cursor: a row, a page or to either end, in the sidebar
//! (which switches scope) or the task list.

use super::scope::{self, Entry};
use super::{App, Effect, Pane};
use crate::action::Action;

impl App {
    pub(super) fn navigate(&mut self, action: Action) -> Vec<Effect> {
        if self.focus == Pane::Detail {
            self.move_detail(action);
            return Vec::new();
        }
        let (index, len) = match self.focus {
            Pane::Sidebar => (self.sidebar_index, self.entries().len()),
            Pane::Tasks | Pane::Detail => (self.task_index, self.tasks.len()),
        };
        let last = len.saturating_sub(1);
        let page = usize::from(self.screen.height.saturating_sub(6).max(1));
        let moved = match action {
            Action::MoveDown => (index + 1).min(last),
            Action::MoveUp => index.saturating_sub(1),
            Action::PageDown if self.focus == Pane::Tasks => self.page_task_index(true, page),
            Action::PageUp if self.focus == Pane::Tasks => self.page_task_index(false, page),
            Action::PageDown => index.saturating_add(page).min(last),
            Action::PageUp => index.saturating_sub(page),
            Action::JumpTop => 0,
            _ => last,
        };
        if self.focus != Pane::Sidebar {
            self.task_index = moved;
            return Vec::new();
        }
        if moved == self.sidebar_index {
            return Vec::new();
        }
        self.open_entry(moved)
    }

    /// Find the next task about one visible page away. Group headings use
    /// a row too, so a page in Planned or Completed doesn't skip tasks.
    fn page_task_index(&self, down: bool, page: usize) -> usize {
        let Some(groups) = scope::task_groups(
            self.shown.as_ref(),
            self.filter.is_some(),
            &self.tasks,
            self.clock.today(),
        ) else {
            return if down {
                self.task_index
                    .saturating_add(page)
                    .min(self.tasks.len().saturating_sub(1))
            } else {
                self.task_index.saturating_sub(page)
            };
        };
        let mut rows = Vec::with_capacity(self.tasks.len());
        let mut row = 0usize;
        for (_, members) in groups {
            row += 1;
            for index in members {
                rows.push((row, index));
                row += 1;
            }
        }
        let Some(current) = rows.iter().find(|(_, index)| *index == self.task_index) else {
            return self.task_index;
        };
        let target = if down {
            current.0.saturating_add(page)
        } else {
            current.0.saturating_sub(page)
        };
        if down {
            rows.iter().find(|(row, _)| *row >= target).or(rows.last())
        } else {
            rows.iter()
                .rev()
                .find(|(row, _)| *row <= target)
                .or(rows.first())
        }
        .map_or(self.task_index, |(_, index)| *index)
    }

    /// Switch to the sidebar's row `index`: paint it from memory if it was
    /// read before, and ask for its seed.
    pub(super) fn open_entry(&mut self, index: usize) -> Vec<Effect> {
        self.sidebar_index = index;
        // A folder's heading shows nothing of its own: the tasks on screen
        // stay until a list or view is chosen.
        let Some(scope) = self.entries().get(index).and_then(Entry::scope) else {
            return Vec::new();
        };
        // A new scope drops the filter, as To Do's search does, and the
        // selection, which holds the old scope's tasks.
        self.filter = None;
        self.filter_error = None;
        self.selection.clear();
        self.wanted = Some(scope);
        if let Some(cached) = self.wanted.as_ref().and_then(|scope| self.cache.get(scope)) {
            self.tasks = cached.clone();
            self.shown = self.wanted.clone();
            self.task_index = 0;
            self.painted_from_cache = true;
        }
        vec![self.seed_now()]
    }
}
