//! Quick add's list suggestion (rung 6b, D-053). While a task typed in
//! the `a` modal is headed for the inbox (no `#List`, not added from a
//! list), the daemon is asked which list it might belong in, once typing
//! has paused, never per key. A likely list shows as a hint, and `Ctrl-l`
//! types `#List` into the text, where it can still be edited. The answer
//! arrives like any other, so the modal never waits for it.

use ms_todo_protocol::{ErrorPayload, ListSuggestion, Request, ResponseData, Scope};

use super::{App, Effect, LineEditor, Mode, Tag};

/// Ticks (250 ms each) of no typing before asking: 250–500 ms after the
/// last key.
pub const QUIET_TICKS: u32 = 2;
/// A title shorter than this says too little to suggest from.
const MIN_TITLE_CHARS: usize = 3;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ListHint {
    /// Suggestions aren't available this session: turned off, or a
    /// daemon without them.
    off: bool,
    /// Ticks since the text last changed, until it's asked about.
    quiet: Option<u32>,
    /// The title asked about, while its answer is on the way. One at a
    /// time, so answers can't arrive out of order.
    asking: Option<String>,
    /// The last answer, and the title it answers: `None` inside when no
    /// list was likely enough.
    answer: Option<(String, Option<ListSuggestion>)>,
}

impl App {
    /// The text changed: ask again once typing pauses.
    pub(super) fn quick_add_typed(&mut self) {
        self.list_hint.quiet = Some(0);
    }

    /// The title a task typed now would have, if it's headed for the
    /// inbox; `None` when it goes to a list or says too little.
    fn inbox_title(&self) -> Option<String> {
        let Mode::Adding { input, parsed } = &self.mode else {
            return None;
        };
        if let Some(Scope::List { id }) = &self.shown
            && !self.lists.iter().any(|list| list.id == *id && list.default)
        {
            return None;
        }
        let title = match parsed {
            Some(parsed) if parsed.list.is_some() => return None,
            Some(parsed) => parsed.title.clone(),
            None => input.text().trim().to_owned(),
        };
        (title.chars().count() >= MIN_TITLE_CHARS).then_some(title)
    }

    /// The list to show as a hint: the answer for the title as it is now.
    pub fn list_hint(&self) -> Option<&ListSuggestion> {
        let (title, list) = self.list_hint.answer.as_ref()?;
        list.as_ref()
            .filter(|_| self.inbox_title().as_ref() == Some(title))
    }

    /// On each tick: once typing has paused, ask about the title, unless
    /// it's been answered or asked about already.
    pub(super) fn tick_list_hint(&mut self) -> Vec<Effect> {
        let hint = &mut self.list_hint;
        let Some(quiet) = hint.quiet.as_mut() else {
            return Vec::new();
        };
        *quiet += 1;
        if hint.off || hint.asking.is_some() || *quiet < QUIET_TICKS {
            return Vec::new();
        }
        hint.quiet = None;
        let Some(title) = self.inbox_title() else {
            return Vec::new();
        };
        if self
            .list_hint
            .answer
            .as_ref()
            .is_some_and(|(answered, _)| *answered == title)
        {
            return Vec::new();
        }
        self.list_hint.asking = Some(title.clone());
        vec![Effect {
            tag: Tag::ListHint,
            request: Request::SuggestList { title },
        }]
    }

    pub(super) fn list_hint_answered(&mut self, result: Result<ResponseData, ErrorPayload>) {
        let Some(title) = self.list_hint.asking.take() else {
            return;
        };
        match result {
            Ok(ResponseData::ListSuggestion { suggestion }) => {
                self.list_hint.answer = Some((title, suggestion));
            }
            // Off, or a daemon that doesn't know the request: don't ask
            // again this session. A lost connection is only this answer.
            Err(error) if error.kind != "daemon_unavailable" => self.list_hint.off = true,
            _ => {}
        }
        // Typed on while it was asked: ask about the text as it is now.
        self.list_hint.quiet = Some(QUIET_TICKS);
    }

    /// `Ctrl-l`: type the suggested list into the text as `#List`.
    pub(super) fn accept_list_hint(&mut self) {
        let Some(token) = self
            .list_hint()
            .map(|list| ms_todo_nlp::list_token(&list.list_name))
        else {
            return;
        };
        if let Mode::Adding { input, .. } = &mut self.mode {
            let text = format!("{} {token}", input.text().trim_end());
            *input = LineEditor::single_at(&text, text.chars().count());
        }
        self.reread_quick_add();
    }
}

#[cfg(test)]
mod tests;
