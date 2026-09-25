//! `a`: quick add. The text is read by `ms_todo_nlp` on every key, as the
//! CLI's `tasks add` reads it, so the modal can highlight what it
//! recognised and preview the task before Enter. `Ctrl-r` takes the text
//! literally instead; Tab completes a `#List` or an `@label`.
//!
//! Where the task goes: a `#List` typed, else the list on screen, else
//! "Tasks" (D-022); from a smart view, with what puts it in the view
//! unless the text says otherwise.

use ms_todo_nlp::{DeterministicParser, ListRef, ParsedTask, QuickAddContext, QuickAddParser};
use ms_todo_protocol::{ErrorPayload, Importance, NewTask, Request, ResponseData, Scope};
use serde_json::Value;

use super::{App, Effect, Level, LineEditor, Mode, Tag, Write};

/// Graph's path for the user's Outlook categories.
const CATEGORIES: &str = "/me/outlook/masterCategories";

/// The categories `@` knows: asked for once, on the first `a`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Categories {
    #[default]
    Unasked,
    Asking,
    /// Read, or `None` when Graph couldn't be asked: then no label is
    /// called unknown.
    Known(Option<Vec<String>>),
}

impl App {
    /// `a`: the modal, and the categories if they haven't been asked for.
    pub(super) fn start_add(&mut self) -> Vec<Effect> {
        self.mode = Mode::Adding {
            input: LineEditor::single(""),
            parsed: Some(self.read_quick_add("")),
        };
        if self.categories != Categories::Unasked {
            return Vec::new();
        }
        self.categories = Categories::Asking;
        vec![Effect {
            tag: Tag::Categories,
            request: Request::RawGet {
                path: CATEGORIES.into(),
            },
        }]
    }

    pub(super) fn categories_answered(&mut self, result: Result<ResponseData, ErrorPayload>) {
        let names = match result {
            Ok(ResponseData::Raw { body }) => Some(
                body.get("value")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|category| category.get("displayName")?.as_str())
                    .map(str::to_owned)
                    .collect(),
            ),
            _ => None,
        };
        self.categories = Categories::Known(names);
        self.reread_quick_add();
    }

    /// What `text` reads as, now.
    fn read_quick_add(&self, text: &str) -> ParsedTask {
        let lists = self.list_refs();
        let known = match &self.categories {
            Categories::Known(known) => known.as_deref(),
            _ => None,
        };
        let ctx = QuickAddContext {
            when: self.parse_context(),
            lists: &lists,
            categories: known,
            due: None,
        };
        DeterministicParser.parse(text, &ctx)
    }

    /// Read the text again, unless it's taken literally: after a key, the
    /// categories, or a tick (so `9am` typed at 08:59 moves on).
    pub(super) fn reread_quick_add(&mut self) {
        let Mode::Adding {
            input,
            parsed: Some(_),
        } = &self.mode
        else {
            return;
        };
        let fresh = self.read_quick_add(&input.text());
        if let Mode::Adding { parsed, .. } = &mut self.mode {
            *parsed = Some(fresh);
        }
    }

    /// `Ctrl-r`: take this text literally, or read it again.
    pub(super) fn toggle_parse(&mut self) {
        let Mode::Adding { input, parsed } = &self.mode else {
            return;
        };
        let fresh = parsed.is_none().then(|| self.read_quick_add(&input.text()));
        if let Mode::Adding { parsed, .. } = &mut self.mode {
            *parsed = fresh;
        }
    }

    fn list_refs(&self) -> Vec<ListRef> {
        self.lists
            .iter()
            .map(|list| ListRef {
                id: list.id.clone(),
                name: list.name.clone(),
            })
            .collect()
    }

    /// The list a task added now goes to, by name: for the modal's title.
    pub fn add_target(&self) -> String {
        let typed = match &self.mode {
            Mode::Adding {
                parsed: Some(parsed),
                ..
            } => parsed.list.as_ref().map(|list| list.name.clone()),
            _ => None,
        };
        typed.unwrap_or_else(|| match &self.shown {
            Some(Scope::List { id }) => self.list_name(id).unwrap_or("Tasks").to_owned(),
            _ => "Tasks".to_owned(),
        })
    }

    /// Tab: finish the `#List` or `@label` the cursor is at the end of, to
    /// the longest text every match shares. A list's name with a space in
    /// it is quoted.
    pub(super) fn complete_quick_add(&mut self) {
        let Mode::Adding { input, .. } = &self.mode else {
            return;
        };
        let text = input.text();
        let cursor = input.cursor().1;
        let at: usize = text.chars().take(cursor).map(char::len_utf8).sum();
        let before = &text[..at];
        // Byte offsets on char boundaries: a space can be wider than a byte
        // (U+3000, U+00A0).
        let start = before
            .char_indices()
            .rfind(|(_, ch)| ch.is_whitespace())
            .map_or(0, |(at, space)| at + space.len_utf8());
        let word = &before[start..];
        let mut chars = word.chars();
        let (sigil, rest) = match chars.next() {
            Some(sigil @ ('#' | '@')) => (sigil, chars.as_str()),
            _ => return,
        };
        let typed = rest.trim_start_matches('"');
        let names: Vec<String> = if sigil == '#' {
            self.lists.iter().map(|list| list.name.clone()).collect()
        } else {
            self.known_labels()
        };
        let typed_lower = typed.to_lowercase();
        let matching: Vec<&String> = names
            .iter()
            .filter(|name| name.to_lowercase().starts_with(&typed_lower))
            .collect();
        let Some(shared) = shared_prefix(&matching) else {
            return;
        };
        let whole = matching.len() == 1;
        let completed = if shared.contains(char::is_whitespace) || rest.starts_with('"') {
            format!("{sigil}\"{shared}{}", if whole { "\" " } else { "" })
        } else {
            format!("{sigil}{shared}{}", if whole { " " } else { "" })
        };
        let new_text = format!("{}{completed}{}", &text[..start], &text[at..]);
        let column = text[..start].chars().count() + completed.chars().count();
        if let Mode::Adding { input, .. } = &mut self.mode {
            *input = LineEditor::single_at(&new_text, column);
        }
        self.reread_quick_add();
        self.quick_add_typed();
    }

    /// The labels Tab offers: the user's categories, else those on the
    /// tasks read so far.
    fn known_labels(&self) -> Vec<String> {
        if let Categories::Known(Some(known)) = &self.categories {
            return known.clone();
        }
        let mut seen: Vec<String> = self
            .tasks
            .iter()
            .flat_map(|task| task.categories.iter().cloned())
            .collect();
        seen.sort_unstable();
        seen.dedup();
        seen
    }

    /// Enter: add the task as read, or as typed with parsing off. Nothing
    /// is sent when no title is left.
    pub(super) fn submit_add(&mut self) -> Vec<Effect> {
        let Mode::Adding { input, parsed, .. } = &self.mode else {
            return Vec::new();
        };
        let (text, parsed) = (input.text(), parsed.clone());
        if text.trim().is_empty() || self.still_loading() {
            self.mode = Mode::Normal;
            return Vec::new();
        }
        if parsed
            .as_ref()
            .is_some_and(|parsed| parsed.title.is_empty())
        {
            self.show(
                Level::Error,
                "Nothing is left for the title; add words, or Ctrl-r to take the text as typed",
            );
            return Vec::new();
        }
        self.mode = Mode::Normal;
        let task = self.new_task(text.trim(), parsed.as_ref());
        vec![Effect {
            tag: Tag::Write(Write::Add),
            request: Request::AddTask {
                task,
                dry_run: false,
                op_id: None,
                idempotency_key: None,
            },
        }]
    }

    /// The task to send: what the text says, over the view's defaults.
    fn new_task(&self, text: &str, parsed: Option<&ParsedTask>) -> NewTask {
        let today = || {
            self.clock
                .today()
                .format(ms_todo_core::DATE_FORMAT)
                .to_string()
        };
        // A list gets the task; a view adds it to the default list with
        // what puts it in the view, as To Do does.
        let my_day = self.shown == Some(Scope::MyDay);
        let (list, importance, due) = match &self.shown {
            Some(Scope::List { id }) => (Some(id.clone()), None, None),
            Some(Scope::Important) => (None, Some(Importance::High), None),
            Some(Scope::Planned) => (None, None, Some(today())),
            _ => (None, None, None),
        };
        let Some(parsed) = parsed else {
            return NewTask {
                title: text.to_owned(),
                list,
                due,
                importance,
                my_day,
                ..NewTask::default()
            };
        };
        let date = |date: chrono::NaiveDate| date.format(ms_todo_core::DATE_FORMAT).to_string();
        NewTask {
            title: parsed.title.clone(),
            list: parsed.list.as_ref().map(|list| list.id.clone()).or(list),
            due: parsed.due.map(date).or(due),
            reminder: parsed
                .reminder
                .map(|at| at.format(ms_todo_core::REMINDER_FORMAT).to_string()),
            importance: parsed
                .importance
                .map(super::edit::protocol_importance)
                .or(importance),
            body: None,
            start: parsed.start.map(date),
            recurrence: parsed
                .recurrence
                .as_ref()
                .map(ms_todo_nlp::Recurrence::to_graph),
            categories: parsed.categories.clone(),
            my_day: parsed.my_day || my_day,
        }
    }
}

/// The longest start every name shares, keeping the first name's case;
/// `None` for no names.
fn shared_prefix(names: &[&String]) -> Option<String> {
    let (first, rest) = names.split_first()?;
    let mut length = first.len();
    for name in rest {
        length = first
            .char_indices()
            .zip(name.chars())
            .take_while(|((_, one), other)| one.to_lowercase().eq(other.to_lowercase()))
            .last()
            .map_or(0, |((at, ch), _)| at + ch.len_utf8())
            .min(length);
    }
    Some(first[..length].to_owned())
}

#[cfg(test)]
mod tests;
