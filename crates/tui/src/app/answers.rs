//! Asking the daemon for seeds, and taking in what it sends back: seeds,
//! write answers and events.

use chrono::NaiveDate;
use ms_todo_protocol::{ErrorPayload, Event, Request, ResponseData, Scope, Seed, SyncState};

use super::scope::{Entry, Shown, SidebarList, VIEWS, belongs, order};
use super::{App, Effect, Level, Mode, Tag, Task, Write, search_query};

impl App {
    /// Read every smart view ahead once, after the first seed is drawn.
    fn prefetch_views(&self) -> Vec<Effect> {
        VIEWS
            .iter()
            .filter(|view| !self.cache.contains_key(view))
            .map(|view| Effect {
                tag: Tag::Prefetch,
                request: Request::Seed {
                    scope: Some(view.clone()),
                    search: None,
                    include_deferred: self.show_deferred,
                    semantic: false,
                },
            })
            .collect()
    }

    /// Ask for the current scope's seed now, superseding any in flight.
    pub(super) fn seed_now(&mut self) -> Effect {
        self.seeds.latest += 1;
        self.seeds.in_flight = true;
        self.seeds.again = false;
        Effect {
            tag: Tag::Seed(self.seeds.latest),
            request: Request::Seed {
                scope: self.wanted.clone(),
                include_deferred: self.show_deferred,
                search: self.filter.as_deref().map(|text| {
                    if self.semantic_filter {
                        text.trim().to_owned()
                    } else {
                        search_query(text)
                    }
                }),
                semantic: self.semantic_filter && self.filter.is_some(),
            },
        }
    }

    /// The cache changed: read it again, once the seed in flight lands.
    pub(super) fn reseed(&mut self) -> Vec<Effect> {
        if self.seeds.in_flight {
            self.seeds.again = true;
            Vec::new()
        } else {
            vec![self.seed_now()]
        }
    }

    pub(super) fn answered(
        &mut self,
        tag: Tag,
        result: Result<ResponseData, ErrorPayload>,
    ) -> Vec<Effect> {
        match (tag, result) {
            (Tag::Seed(number), result) => {
                if number != self.seeds.latest {
                    return Vec::new();
                }
                self.seeds.in_flight = false;
                let mut effects = match result {
                    Ok(ResponseData::Seed(seed)) => {
                        // The filter as typed searched: any earlier reason
                        // it couldn't (such as the model loading) is stale.
                        self.filter_error = None;
                        let first = !self.seeded;
                        self.apply_seed(*seed);
                        if first && self.lists_ready {
                            self.prefetch_views()
                        } else {
                            Vec::new()
                        }
                    }
                    Ok(_) => {
                        self.show(Level::Error, "The daemon sent an unexpected answer");
                        Vec::new()
                    }
                    Err(error) => self.seed_failed(error),
                };
                if std::mem::take(&mut self.seeds.again) && effects.is_empty() {
                    effects.push(self.seed_now());
                }
                effects
            }
            (Tag::Prefetch, Ok(ResponseData::Seed(seed))) => {
                if let Some(scope) = seed.scope.clone() {
                    let mut tasks = seed_tasks(&seed);
                    order(&scope, false, &mut tasks);
                    self.cache.insert(scope, tasks);
                }
                Vec::new()
            }
            // Only a head start; the scope's own seed says what's wrong.
            (Tag::Prefetch, _) => Vec::new(),
            (Tag::Diagnostics(part), result) => {
                self.diagnosed(part, result);
                Vec::new()
            }
            // Without them, no label is called unknown; nothing to say.
            (Tag::Categories, result) => {
                self.categories_answered(result);
                Vec::new()
            }
            // Only a hint: without one, nothing to say.
            (Tag::ListHint, result) => {
                self.list_hint_answered(result);
                Vec::new()
            }
            (Tag::Download, result) => {
                self.downloaded(result);
                Vec::new()
            }
            (Tag::Context, result) => self.context_switched(result),
            (Tag::TriageInbox, result) => self.triage_inbox(result),
            (Tag::TriageSuggest, result) => self.triage_suggested(result),
            (Tag::Write(write), Ok(ResponseData::Applied(applied))) => {
                self.apply_write(write, &applied.items);
                if write == Write::Move {
                    self.moved(&applied);
                }
                if write == Write::MyDay {
                    self.my_day_changed(&applied);
                }
                if write == Write::Nag {
                    self.nag_changed(&applied);
                }
                if write == Write::Edit && applied.items.len() > 1 {
                    let count = applied.items.len();
                    self.show(
                        Level::Info,
                        &format!("Changed {count} tasks; u puts them all back"),
                    );
                }
                Vec::new()
            }
            (Tag::Folders, Ok(ResponseData::Applied(applied))) => {
                self.apply_list_write(&applied.items);
                Vec::new()
            }
            (Tag::Lists, Ok(ResponseData::Applied(applied))) => self.list_changed(&applied),
            // The sidebar moved when the key was pressed: read the order
            // back from the daemon.
            (Tag::Order, Err(error)) => {
                self.show(Level::Error, &error.message);
                self.reseed()
            }
            (Tag::Undo, Ok(ResponseData::Applied(applied))) => {
                let text = match applied.refused.as_slice() {
                    [] => "Undone".to_owned(),
                    refused => {
                        let titles: Vec<String> = refused
                            .iter()
                            .map(|task| format!("\"{}\"", task.title))
                            .collect();
                        format!(
                            "Undone, except {}: changed since, so left alone",
                            titles.join(", ")
                        )
                    }
                };
                self.show(Level::Info, &text);
                self.reseed()
            }
            (Tag::Undo, Err(error)) => {
                match (&error.undo_target, error.candidates.is_empty()) {
                    (Some(target), false) => {
                        self.mode = Mode::Picker {
                            target: target.clone(),
                            candidates: error.candidates,
                            index: 0,
                        };
                    }
                    _ => self.show(Level::Error, &error.message),
                }
                Vec::new()
            }
            (_, Err(error)) => {
                self.show(Level::Error, &error.message);
                Vec::new()
            }
            (_, Ok(_)) => Vec::new(),
        }
    }

    fn seed_failed(&mut self, error: ErrorPayload) -> Vec<Effect> {
        if error.kind == "auth_required" && !self.seeded && self.sign_in_command.is_some() {
            self.sign_in_required = true;
            return Vec::new();
        }
        let not_ready = self.semantic_filter && error.kind == "network";
        if self.filter.is_some() && (error.kind == "invalid_input" || not_ready) {
            // Mid-typing, like an unclosed quote, semantic search off, or
            // its model still loading: keep the last results.
            self.filter_error = Some(error.message);
            return Vec::new();
        }
        if error.kind == "not_found" && matches!(self.wanted, Some(Scope::List { .. })) {
            // The list was deleted elsewhere: back to the default list.
            self.show(Level::Info, "That list is gone; showing Tasks");
            self.wanted = None;
            return vec![self.seed_now()];
        }
        self.show(Level::Error, &error.message);
        Vec::new()
    }

    fn apply_seed(&mut self, seed: Seed) {
        self.sign_in_required = false;
        let keep = self.selected().map(|task| task.id.clone());
        let row = self.entries().get(self.sidebar_index).cloned();
        let tasks = seed_tasks(&seed);
        self.lists = seed
            .lists
            .iter()
            .filter_map(SidebarList::from_entity)
            .collect();
        self.counts = seed.counts;
        let switched = self.active_context != seed.context;
        if switched {
            // Another context's rows, read before, aren't this one's.
            self.cache.clear();
        }
        self.active_context = seed.context;
        self.context_names = seed.contexts;
        self.activity = seed.activity;
        self.outbox = seed.outbox;
        self.lists_ready = seed.lists_sync.state == SyncState::Ready;
        self.tasks_ready = seed.sync.state == SyncState::Ready;
        self.seeded = true;
        if let Some(scope) = &seed.scope {
            self.wanted = Some(scope.clone());
        }
        if switched {
            self.left_context();
        }
        self.place_sidebar_cursor(row.filter(|row| matches!(row, Entry::Folder { .. })));
        let changed_scope = self.shown != seed.scope;
        if changed_scope {
            self.selection.clear();
        }
        if let Some(my_day) = &seed.my_day {
            self.my_day_date =
                NaiveDate::parse_from_str(&my_day.date, ms_todo_core::DATE_FORMAT).ok();
        }
        self.tasks = tasks;
        self.shown = seed.scope;
        if let Some(shown) = &self.shown {
            order(shown, self.filter.is_some(), &mut self.tasks);
            if self.filter.is_none() {
                self.cache.insert(shown.clone(), self.tasks.clone());
            }
        }
        self.task_index = match keep.filter(|_| !changed_scope) {
            Some(id) => self
                .tasks
                .iter()
                .position(|task| task.id == id)
                .unwrap_or(self.task_index),
            None => 0,
        }
        .min(self.tasks.len().saturating_sub(1));
        self.prune_selection();
    }

    /// Draw a write's `Applied` answer at once, before the event that
    /// follows it: the row moves, appears or goes, marked pending.
    fn apply_write(&mut self, write: Write, items: &[ms_todo_protocol::Entity]) {
        let Some(shown) = self.shown.clone() else {
            return;
        };
        let my_day = self.my_day();
        let rule = Shown {
            my_day,
            today: self.clock.today(),
            // A search finds deferred tasks too.
            deferred: self.show_deferred || self.filter.is_some(),
        };
        for mut task in items.iter().filter_map(Task::from_entity) {
            let at = self.tasks.iter().position(|row| row.id == task.id);
            if let Some(at) = at {
                // Still a suggestion, and Next's reason, until the view is
                // read again.
                if task.my_day != Some(my_day) {
                    task.suggestion = self.tasks[at].suggestion.clone();
                }
                task.why = task.why.take().or_else(|| self.tasks[at].why.clone());
            }
            match (write, at) {
                (Write::Delete, Some(at)) => {
                    self.tasks.remove(at);
                }
                (Write::Delete, None) => {}
                // A filtered list is the search's to decide.
                (Write::Add, _) if self.filter.is_some() => {}
                (Write::Add, _) if belongs(&shown, &task, rule) => {
                    let id = task.id.clone();
                    let open = self.tasks.iter().filter(|row| !row.completed).count();
                    self.tasks.insert(open, task);
                    self.task_index = self
                        .tasks
                        .iter()
                        .position(|row| row.id == id)
                        .unwrap_or(self.task_index);
                }
                (Write::Add, _) => {
                    let name = self.list_name(&task.list_id).unwrap_or("Tasks").to_owned();
                    self.show(Level::Info, &format!("Added to {name}"));
                }
                (_, Some(at)) if belongs(&shown, &task, rule) => self.tasks[at] = task,
                (_, Some(at)) => {
                    self.tasks.remove(at);
                }
                (_, None) => {}
            }
        }
        if write != Write::Add {
            // The cursor stays where it was, so `x` down a list completes
            // one task after another.
            order(&shown, self.filter.is_some(), &mut self.tasks);
        }
        self.task_index = self.task_index.min(self.tasks.len().saturating_sub(1));
        self.prune_selection();
    }

    pub(super) fn event(&mut self, event: Event) -> Vec<Effect> {
        match event {
            Event::EntityChanged(_) | Event::ResyncNeeded => self.reseed(),
            Event::WriteRejected(rejected) => {
                self.show(
                    Level::Error,
                    &format!(
                        "Microsoft To Do rejected a change, which was undone here: {}",
                        rejected.error.message
                    ),
                );
                self.rejections
                    .insert(rejected.task_id, rejected.error.message);
                self.reseed()
            }
            Event::SyncState(activity) => {
                let finished = !activity.in_progress;
                self.activity = activity;
                if finished && (!self.lists_ready || !self.tasks_ready) {
                    self.reseed()
                } else {
                    Vec::new()
                }
            }
            // Only a search by meaning ranks by the index: re-run it, as
            // the model may have just loaded or the tasks been re-embedded.
            Event::IndexChanged if self.semantic_filter && self.filter.is_some() => self.reseed(),
            Event::IndexChanged | Event::SyncProgress(_) | Event::Unknown => Vec::new(),
        }
    }
}

/// A seed's tasks as the TUI shows them: for My Day, its suggestions
/// after its tasks.
fn seed_tasks(seed: &Seed) -> Vec<Task> {
    let suggestions = seed
        .my_day
        .iter()
        .flat_map(|my_day| my_day.suggestions.iter());
    seed.tasks
        .iter()
        .chain(suggestions)
        .filter_map(Task::from_entity)
        .collect()
}
