//! "Suggest lists for inbox" (D-066): the palette command that walks the
//! inbox's open tasks one by one, asks the daemon which list each might
//! belong in (`SuggestList`, as `tasks suggest-list` does), and moves it
//! there with one key, or skips it. One request at a time, nothing read
//! ahead: the provider is asked about a task only once it's on screen.

use ms_todo_protocol::{ErrorPayload, ListSuggestion, Request, ResponseData, Scope, TaskChange};

use super::{App, Effect, Level, Mode, Tag, Task, Write, change};

/// A task waiting to be triaged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    pub id: String,
    pub title: String,
}

/// Where the task on screen stands.
#[derive(Clone, Debug, PartialEq)]
pub enum Suggestion {
    /// The inbox's tasks are on their way.
    Loading,
    /// The daemon is asked.
    Asking,
    Found(ListSuggestion),
    /// No list is likely enough; it stays in the inbox unless moved by hand.
    Nothing,
    /// Why the daemon couldn't suggest: suggestions are off (its message
    /// says how to turn them on), or the provider failed.
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Triage {
    /// The inbox's ID, so a suggestion of the inbox itself is none.
    pub inbox: String,
    pub tasks: Vec<Pending>,
    /// Which of `tasks` is on screen.
    pub at: usize,
    pub suggestion: Suggestion,
    /// The task whose suggestion is on its way, while one is: an answer
    /// for a task already skipped is dropped, and the one on screen asked
    /// about instead.
    asked: Option<usize>,
    pub moved: usize,
    pub skipped: usize,
}

impl Triage {
    pub fn current(&self) -> Option<&Pending> {
        self.tasks.get(self.at)
    }
}

impl App {
    /// The palette's "Suggest lists for inbox": read the inbox, then
    /// start on its first task.
    pub(super) fn start_triage(&mut self) -> Vec<Effect> {
        if !self.lists_ready {
            self.show(
                Level::Info,
                "Still syncing your lists; try again in a moment",
            );
            return Vec::new();
        }
        let Some(inbox) = self.lists.iter().find(|list| list.default) else {
            self.show(Level::Info, "There's no Tasks list to triage yet");
            return Vec::new();
        };
        let inbox = inbox.id.clone();
        self.mode = Mode::Triage(Triage {
            inbox: inbox.clone(),
            tasks: Vec::new(),
            at: 0,
            suggestion: Suggestion::Loading,
            asked: None,
            moved: 0,
            skipped: 0,
        });
        vec![Effect {
            tag: Tag::TriageInbox,
            request: Request::Seed {
                scope: Some(Scope::List { id: inbox }),
                search: None,
                include_deferred: false,
                semantic: false,
            },
        }]
    }

    /// The inbox's tasks: its open ones are the queue.
    pub(super) fn triage_inbox(
        &mut self,
        result: Result<ResponseData, ErrorPayload>,
    ) -> Vec<Effect> {
        let Mode::Triage(triage) = &mut self.mode else {
            return Vec::new();
        };
        let seed = match result {
            Ok(ResponseData::Seed(seed)) => seed,
            Ok(_) => return self.end_triage("The daemon sent an unexpected answer"),
            Err(error) => return self.end_triage(&error.message),
        };
        triage.tasks = seed
            .tasks
            .iter()
            .filter_map(Task::from_entity)
            .filter(|task| !task.completed)
            .map(|task| Pending {
                id: task.id,
                title: task.title,
            })
            .collect();
        if triage.tasks.is_empty() {
            return self.end_triage("Nothing to triage: the inbox has no open tasks");
        }
        self.ask_triage()
    }

    /// Ask about the task on screen, unless an answer is on its way.
    fn ask_triage(&mut self) -> Vec<Effect> {
        let Mode::Triage(triage) = &mut self.mode else {
            return Vec::new();
        };
        triage.suggestion = Suggestion::Asking;
        if triage.asked.is_some() {
            return Vec::new();
        }
        let Some(task) = triage.current() else {
            return Vec::new();
        };
        let title = task.title.clone();
        triage.asked = Some(triage.at);
        vec![Effect {
            tag: Tag::TriageSuggest,
            request: Request::SuggestList { title },
        }]
    }

    pub(super) fn triage_suggested(
        &mut self,
        result: Result<ResponseData, ErrorPayload>,
    ) -> Vec<Effect> {
        let Mode::Triage(triage) = &mut self.mode else {
            return Vec::new();
        };
        if triage.asked.take() != Some(triage.at) {
            return self.ask_triage();
        }
        triage.suggestion = match result {
            Ok(ResponseData::ListSuggestion {
                suggestion: Some(list),
            }) if list.list_id != triage.inbox => Suggestion::Found(list),
            Ok(ResponseData::ListSuggestion { .. }) => Suggestion::Nothing,
            Ok(_) => Suggestion::Failed("The daemon sent an unexpected answer".into()),
            Err(error) => Suggestion::Failed(error.message),
        };
        Vec::new()
    }

    /// Enter or `m`: move the task to the suggested list, then the next.
    pub(super) fn triage_move(&mut self) -> Vec<Effect> {
        let Mode::Triage(triage) = &mut self.mode else {
            return Vec::new();
        };
        let (Suggestion::Found(list), Some(task)) = (&triage.suggestion, triage.current()) else {
            return Vec::new();
        };
        let effect = change(
            Write::Move,
            vec![task.id.clone()],
            TaskChange::Move {
                to: list.list_id.clone(),
            },
        );
        triage.moved += 1;
        let mut effects = vec![effect];
        effects.extend(self.next_triage());
        effects
    }

    /// `s`: leave the task in the inbox, then the next.
    pub(super) fn triage_skip(&mut self) -> Vec<Effect> {
        let Mode::Triage(triage) = &mut self.mode else {
            return Vec::new();
        };
        if triage.current().is_none() {
            return Vec::new();
        }
        triage.skipped += 1;
        self.next_triage()
    }

    fn next_triage(&mut self) -> Vec<Effect> {
        let Mode::Triage(triage) = &mut self.mode else {
            return Vec::new();
        };
        triage.at += 1;
        if triage.at < triage.tasks.len() {
            return self.ask_triage();
        }
        self.end_triage("Inbox done")
    }

    /// Esc: stop, saying what was done.
    pub(super) fn stop_triage(&mut self) -> Vec<Effect> {
        self.end_triage("Stopped")
    }

    /// Back to browsing, with `text` and, once any task was seen, how
    /// many were moved and left.
    fn end_triage(&mut self, text: &str) -> Vec<Effect> {
        let Mode::Triage(triage) = std::mem::replace(&mut self.mode, Mode::Normal) else {
            return Vec::new();
        };
        let text = if triage.moved + triage.skipped > 0 {
            format!(
                "{text}: moved {}, left {} in the inbox",
                triage.moved, triage.skipped
            )
        } else {
            text.to_owned()
        };
        self.show(Level::Info, &text);
        Vec::new()
    }
}

#[cfg(test)]
pub(crate) mod tests;
