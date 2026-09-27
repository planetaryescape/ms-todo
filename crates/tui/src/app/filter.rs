//! What a view leaves out: `/` narrows the task list as each key is
//! typed, by the daemon's search, and `z` shows or hides deferred and
//! Someday tasks.

use super::{App, Effect, Level, Mode};

impl App {
    /// After the filter's text changed, search again.
    pub(super) fn filter_typed(&mut self) -> Vec<Effect> {
        match &self.mode {
            Mode::Filtering { input } => {
                let text = input.text();
                self.set_filter(Some(text))
            }
            _ => Vec::new(),
        }
    }

    pub(super) fn set_filter(&mut self, text: Option<String>) -> Vec<Effect> {
        let text = text.filter(|text| !text.trim().is_empty());
        if text == self.filter {
            return Vec::new();
        }
        self.filter = text;
        self.filter_error = None;
        vec![self.seed_now()]
    }

    /// `z`: every view shows deferred and Someday tasks, or leaves them
    /// out again. Each view's rows read before are for the other way.
    pub(super) fn toggle_deferred(&mut self) -> Vec<Effect> {
        self.show_deferred = !self.show_deferred;
        self.cache.clear();
        let said = if self.show_deferred {
            "Showing deferred and Someday tasks; z hides them"
        } else {
            "Hiding deferred and Someday tasks"
        };
        self.show(Level::Info, said);
        vec![self.seed_now()]
    }
}

/// The filter as typed, as a search: the word being typed matches as a
/// prefix, so the list narrows with each key. A word already ending in
/// `*` or a closing quote, or an operator, is left as it is.
pub fn search_query(text: &str) -> String {
    let trimmed = text.trim_end();
    let typing = trimmed.len() == text.len();
    let last = trimmed.split_whitespace().next_back().unwrap_or_default();
    let is_operator = matches!(last, "AND" | "OR" | "NOT");
    if typing
        && !is_operator
        && last
            .chars()
            .next_back()
            .is_some_and(|ch| ch.is_alphanumeric())
        && !last.starts_with('"')
    {
        format!("{trimmed}*")
    } else {
        trimmed.to_owned()
    }
}
