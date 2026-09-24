// The Elm-style split is adapted from mxr crates/tui/src/app/ @ dfb23d10138b1cfc24f8ea7450d3426e5e4da37a
// (state plus an update function over actions; `ui/` only reads it).
// Changes: `update` returns the requests to send as `Effect`s instead of
// sending them itself, so every action is a pure function of the state,
// tested without a daemon or a terminal; and there is one `App`, not
// mxr's page structs, since 5a has one screen.

//! The TUI's state and the one function that changes it. The runner feeds
//! [`Msg`]s in (keys, the daemon's answers and events, ticks) and sends
//! the [`Effect`]s that come out; `ui` draws the state.
//!
//! The TUI starts from the daemon's `Seed` and reads it again whenever an
//! event says the cache changed (D-031). A write's `Applied` answer is
//! drawn at once; the event that follows it brings the rest (the counts,
//! the sync marker) up to date.

pub mod diagnostics;
pub mod edit;
pub mod palette;
pub mod scope;
mod selection;
pub mod task;

use std::collections::{HashMap, HashSet};

use chrono::{Local, NaiveDate};
use ms_todo_protocol::{
    Candidate, Counts, ErrorPayload, Event, Importance, NewTask, OutboxDepth, Request,
    ResponseData, Scope, Seed, SyncActivity, SyncState, TaskChange,
};
use serde_json::Value;

use crate::action::Action;
use crate::glyphs::Glyphs;
use crate::keybindings::Context;
use diagnostics::{Diagnostics, Part};
use edit::Field;
use scope::{Entry, VIEWS, belongs, order};
pub use task::{SyncMarker, Task};

/// How long a banner stays, in ticks (the runner ticks every 250 ms).
pub const BANNER_TICKS: u32 = 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Sidebar,
    Tasks,
    Detail,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Mode {
    Normal,
    /// Typing a new task's title.
    Adding {
        text: String,
    },
    /// Typing a filter; the list follows each key.
    Filtering {
        text: String,
    },
    /// Typing a new value for one field of a task.
    Editing {
        id: String,
        field: Field,
        text: String,
        /// Why the text can't be sent, shown under it.
        error: Option<String>,
    },
    /// The inline "Delete …? y/n", for one task or the selection.
    ConfirmDelete {
        ids: Vec<String>,
        /// What's deleted, as the question names it: `"Call Sam"` or
        /// `3 tasks`.
        what: String,
    },
    /// The command palette: the query typed, and which match is chosen.
    Palette {
        query: String,
        index: usize,
    },
    Diagnostics,
    /// Undoing a recurring completion: which completed copy to delete.
    Picker {
        target: String,
        candidates: Vec<Candidate>,
        index: usize,
    },
    Help,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Connection {
    Connecting,
    Connected,
    Lost(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Info,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Banner {
    pub text: String,
    pub level: Level,
    pub ticks_left: u32,
}

/// Now, for what's drawn relative to it ("synced 5s ago", Overdue). Set by
/// ticks, so tests pick their own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clock {
    pub unix: i64,
    pub today: NaiveDate,
}

impl Clock {
    pub fn now() -> Self {
        let now = Local::now();
        Self {
            unix: now.timestamp(),
            today: now.date_naive(),
        }
    }
}

/// Which request an answer belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tag {
    /// A `Seed`, numbered: only the latest one asked for is drawn.
    Seed(u64),
    /// A smart view's seed read ahead, so switching to it paints at once.
    Prefetch,
    Write(Write),
    Undo,
    Sync,
    Diagnostics(Part),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Write {
    Add,
    Complete,
    Reopen,
    Edit,
    Delete,
}

/// What goes into [`App::update`]. A seed makes `Response` the big one;
/// messages are moved once each, so boxing it would buy nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq)]
pub enum Msg {
    Action(Action),
    /// A character typed into a prompt.
    Char(char),
    /// Connected to the daemon and subscribed.
    Connected,
    Disconnected(String),
    Response {
        tag: Tag,
        result: Result<ResponseData, ErrorPayload>,
    },
    Event(Event),
    Tick(Clock),
}

/// What comes out: a request for the daemon.
#[derive(Clone, Debug, PartialEq)]
pub struct Effect {
    pub tag: Tag,
    pub request: Request,
}

/// Seeds asked for and answered. A newer seed supersedes one in flight;
/// a change while one is in flight asks again once it lands, so a burst
/// of events costs one read, not one each.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Seeds {
    latest: u64,
    in_flight: bool,
    again: bool,
}

pub struct App {
    pub glyphs: Glyphs,
    /// ms-todo's version, as the title bar and help show it.
    pub version: &'static str,
    pub focus: Pane,
    pub mode: Mode,
    pub connection: Connection,
    /// `(local ID, name)` of every list, in the daemon's order.
    pub lists: Vec<(String, String)>,
    pub counts: Counts,
    pub activity: SyncActivity,
    pub outbox: OutboxDepth,
    /// Whether the lists have synced once.
    pub lists_ready: bool,
    /// Whether any seed has been drawn yet.
    pub seeded: bool,
    pub sidebar_index: usize,
    /// The scope asked for; `None` is the default list before anyone knows
    /// which that is.
    pub wanted: Option<Scope>,
    /// The scope `tasks` came from.
    pub shown: Option<Scope>,
    pub tasks: Vec<Task>,
    pub task_index: usize,
    /// The detail pane's cursor: the field `e` and Enter edit.
    pub detail_field: Field,
    /// The tasks `v` and `V` selected, by ID; all rows of `tasks`.
    pub selection: HashSet<String>,
    pub diagnostics: Diagnostics,
    /// Whether the shown scope has synced once.
    pub tasks_ready: bool,
    /// The applied filter.
    pub filter: Option<String>,
    /// Why the filter as typed can't be searched, such as an unclosed quote.
    pub filter_error: Option<String>,
    pub banner: Option<Banner>,
    /// Why Microsoft To Do rejected a write, by task, for the detail pane.
    pub rejections: HashMap<String, String>,
    pub clock: Clock,
    pub should_quit: bool,
    seeds: Seeds,
    /// Each scope's tasks as last read, unfiltered: switching back to a
    /// scope paints these at once, and its fresh seed replaces them a few
    /// milliseconds later (docs/blueprint/08-tui.md#latency-budget).
    cache: HashMap<Scope, Vec<Task>>,
    /// The last key switched scope and painted the new one from `cache`:
    /// its frame is the view switch (the runner's measurement).
    pub painted_from_cache: bool,
}

impl App {
    pub fn new(glyphs: Glyphs, clock: Clock) -> Self {
        Self {
            glyphs,
            version: env!("CARGO_PKG_VERSION"),
            focus: Pane::Tasks,
            mode: Mode::Normal,
            connection: Connection::Connecting,
            lists: Vec::new(),
            counts: Counts::default(),
            activity: SyncActivity::default(),
            outbox: OutboxDepth::default(),
            lists_ready: false,
            seeded: false,
            sidebar_index: 0,
            wanted: None,
            shown: None,
            tasks: Vec::new(),
            task_index: 0,
            detail_field: Field::default(),
            selection: HashSet::new(),
            diagnostics: Diagnostics::default(),
            tasks_ready: false,
            filter: None,
            filter_error: None,
            banner: None,
            rejections: HashMap::new(),
            clock,
            should_quit: false,
            seeds: Seeds::default(),
            cache: HashMap::new(),
            painted_from_cache: false,
        }
    }

    /// Where keys go now.
    pub fn context(&self) -> Context {
        match self.mode {
            Mode::Adding { .. } | Mode::Filtering { .. } | Mode::Editing { .. } => Context::Prompt,
            Mode::ConfirmDelete { .. } => Context::Confirm,
            Mode::Picker { .. } => Context::Picker,
            Mode::Palette { .. } => Context::Palette,
            Mode::Diagnostics => Context::Diagnostics,
            Mode::Help => Context::Help,
            Mode::Normal => match self.focus {
                Pane::Sidebar => Context::Sidebar,
                Pane::Tasks => Context::Tasks,
                Pane::Detail => Context::Detail,
            },
        }
    }

    /// The sidebar's rows: the smart views, then the lists.
    pub fn entries(&self) -> Vec<Entry> {
        VIEWS
            .iter()
            .cloned()
            .map(Entry::View)
            .chain(self.lists.iter().map(|(id, name)| Entry::List {
                id: id.clone(),
                name: name.clone(),
            }))
            .collect()
    }

    /// The selected task. `None` while loading: the rows on hand belong to
    /// another scope, and no action may take its target from them.
    pub fn selected(&self) -> Option<&Task> {
        if self.loading() {
            return None;
        }
        self.tasks.get(self.task_index)
    }

    /// Whether the scope selected in the sidebar isn't the one the rows
    /// came from yet: its seed is still on the way.
    pub fn loading(&self) -> bool {
        self.wanted.is_some() && self.shown != self.wanted
    }

    /// Refuse a task action while loading, saying why. True when refused.
    fn still_loading(&mut self) -> bool {
        if self.loading() {
            self.show(
                Level::Info,
                "Still loading this list; try again in a moment",
            );
        }
        self.loading()
    }

    /// A list's name by local ID.
    pub fn list_name(&self, id: &str) -> Option<&str> {
        self.lists
            .iter()
            .find(|(list, _)| list == id)
            .map(|(_, name)| name.as_str())
    }

    /// The name of a scope as the panes title it.
    pub fn scope_name(&self, scope: Option<&Scope>) -> String {
        match scope {
            None => "Tasks".into(),
            Some(Scope::List { id }) => self.list_name(id).unwrap_or("List").to_owned(),
            Some(view) => scope::view_name(view).to_owned(),
        }
    }

    /// The name of what the task list shows: the scope asked for, else
    /// the one on screen.
    pub fn view_name(&self) -> String {
        self.scope_name(self.wanted.as_ref().or(self.shown.as_ref()))
    }

    /// The terminal window's title (08-tui.md): which app, and where.
    pub fn window_title(&self) -> String {
        format!("ms-todo \u{2014} {}", self.view_name())
    }

    pub fn update(&mut self, msg: Msg) -> Vec<Effect> {
        match msg {
            Msg::Action(action) => self.act(action),
            Msg::Char(ch) => self.type_char(ch),
            Msg::Connected => {
                self.connection = Connection::Connected;
                vec![self.seed_now()]
            }
            Msg::Disconnected(why) => {
                self.connection = Connection::Lost(why);
                // Whatever was in flight is lost with the connection.
                self.seeds.in_flight = false;
                self.seeds.again = false;
                Vec::new()
            }
            Msg::Response { tag, result } => self.answered(tag, result),
            Msg::Event(event) => self.event(event),
            Msg::Tick(clock) => {
                self.clock = clock;
                if let Some(banner) = &mut self.banner {
                    banner.ticks_left = banner.ticks_left.saturating_sub(1);
                    if banner.ticks_left == 0 {
                        self.banner = None;
                    }
                }
                Vec::new()
            }
        }
    }

    fn act(&mut self, action: Action) -> Vec<Effect> {
        match (&mut self.mode, action) {
            (
                Mode::Picker {
                    index, candidates, ..
                },
                Action::MoveDown,
            ) => {
                *index = (*index + 1).min(candidates.len().saturating_sub(1));
                Vec::new()
            }
            (Mode::Picker { index, .. }, Action::MoveUp) => {
                *index = index.saturating_sub(1);
                Vec::new()
            }
            (Mode::Picker { .. }, Action::Submit) => self.pick_copy(),
            (Mode::Palette { .. }, Action::MoveDown | Action::MoveUp) => {
                self.palette_step(action == Action::MoveDown);
                Vec::new()
            }
            (Mode::Palette { .. }, Action::Submit) => self.run_palette(),
            (Mode::Palette { query, index }, Action::Backspace) => {
                query.pop();
                *index = 0;
                Vec::new()
            }
            (Mode::Diagnostics, Action::MoveDown | Action::MoveUp) => {
                self.scroll_diagnostics(action == Action::MoveDown);
                Vec::new()
            }
            (Mode::Diagnostics, Action::Refresh) => self.refresh_diagnostics(),
            (Mode::Editing { .. }, Action::Submit) => self.submit_edit(),
            (Mode::Editing { text, error, .. }, Action::Backspace) => {
                text.pop();
                *error = None;
                Vec::new()
            }
            (Mode::Adding { .. }, Action::Submit) => self.submit_add(),
            (Mode::Filtering { .. }, Action::Submit) => {
                self.mode = Mode::Normal;
                self.focus = Pane::Tasks;
                Vec::new()
            }
            (Mode::Filtering { .. }, Action::Cancel) => {
                self.mode = Mode::Normal;
                self.set_filter(None)
            }
            (Mode::Adding { text } | Mode::Filtering { text }, Action::Backspace) => {
                text.pop();
                self.filter_typed()
            }
            (Mode::ConfirmDelete { ids, .. }, Action::Confirm) => {
                let ids = std::mem::take(ids);
                self.mode = Mode::Normal;
                self.selection.clear();
                vec![change(Write::Delete, ids, TaskChange::Delete)]
            }
            (_, Action::Cancel) => {
                self.mode = Mode::Normal;
                Vec::new()
            }
            (Mode::Normal, action) => self.browse(action),
            _ => Vec::new(),
        }
    }

    fn browse(&mut self, action: Action) -> Vec<Effect> {
        match action {
            Action::MoveDown | Action::MoveUp | Action::JumpTop | Action::JumpBottom => {
                self.navigate(action)
            }
            Action::FocusLeft => {
                self.focus = match self.focus {
                    Pane::Detail => Pane::Tasks,
                    _ => Pane::Sidebar,
                };
                Vec::new()
            }
            Action::FocusRight => {
                self.focus = match self.focus {
                    Pane::Sidebar => Pane::Tasks,
                    _ => Pane::Detail,
                };
                Vec::new()
            }
            Action::FocusNext => {
                self.focus = match self.focus {
                    Pane::Sidebar => Pane::Tasks,
                    Pane::Tasks => Pane::Detail,
                    Pane::Detail => Pane::Sidebar,
                };
                Vec::new()
            }
            Action::Add if self.still_loading() => Vec::new(),
            Action::Add => {
                if self.lists_ready {
                    self.mode = Mode::Adding {
                        text: String::new(),
                    };
                } else {
                    self.show(
                        Level::Info,
                        "Still syncing your lists; try again in a moment",
                    );
                }
                Vec::new()
            }
            Action::ToggleComplete | Action::Delete if self.still_loading() => Vec::new(),
            Action::ToggleComplete => self.toggle_complete(),
            Action::Delete => {
                let targets = self.targets();
                let what = match targets.as_slice() {
                    [] => return Vec::new(),
                    [task] => format!("\"{}\"", task.title),
                    many => format!("{} tasks", many.len()),
                };
                let ids = targets.iter().map(|task| task.id.clone()).collect();
                self.mode = Mode::ConfirmDelete { ids, what };
                Vec::new()
            }
            Action::Edit => self.start_edit(),
            Action::ToggleSelect => {
                self.toggle_select();
                Vec::new()
            }
            Action::SelectAll => {
                self.select_all();
                Vec::new()
            }
            Action::Palette => {
                self.open_palette();
                Vec::new()
            }
            Action::Diagnostics => self.open_diagnostics(),
            Action::Undo => vec![Effect {
                tag: Tag::Undo,
                request: Request::Undo {
                    target: None,
                    copy: None,
                    op_id: None,
                    idempotency_key: None,
                },
            }],
            Action::Filter => {
                self.mode = Mode::Filtering {
                    text: self.filter.clone().unwrap_or_default(),
                };
                Vec::new()
            }
            Action::Clear if !self.selection.is_empty() => {
                self.selection.clear();
                Vec::new()
            }
            Action::Clear if self.filter.is_some() => self.set_filter(None),
            Action::Sync => vec![Effect {
                tag: Tag::Sync,
                request: Request::Sync { wait: false },
            }],
            Action::Help => {
                self.mode = Mode::Help;
                Vec::new()
            }
            Action::Quit => {
                self.should_quit = true;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn navigate(&mut self, action: Action) -> Vec<Effect> {
        if self.focus == Pane::Detail {
            self.detail_field = match action {
                Action::MoveDown => self.detail_field.step(1),
                Action::MoveUp => self.detail_field.step(-1),
                Action::JumpTop => Field::ALL[0],
                Action::JumpBottom => Field::ALL[Field::ALL.len() - 1],
                _ => self.detail_field,
            };
            return Vec::new();
        }
        let (index, len) = match self.focus {
            Pane::Sidebar => (self.sidebar_index, self.entries().len()),
            Pane::Tasks | Pane::Detail => (self.task_index, self.tasks.len()),
        };
        let last = len.saturating_sub(1);
        let moved = match action {
            Action::MoveDown => (index + 1).min(last),
            Action::MoveUp => index.saturating_sub(1),
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

    /// Switch to the sidebar's row `index`: paint it from memory if it was
    /// read before, and ask for its seed.
    fn open_entry(&mut self, index: usize) -> Vec<Effect> {
        self.sidebar_index = index;
        // A new scope drops the filter, as To Do's search does, and the
        // selection, which holds the old scope's tasks.
        self.filter = None;
        self.filter_error = None;
        self.selection.clear();
        self.wanted = self.entries().get(index).map(Entry::scope);
        if let Some(cached) = self.wanted.as_ref().and_then(|scope| self.cache.get(scope)) {
            self.tasks = cached.clone();
            self.shown = self.wanted.clone();
            self.task_index = 0;
            self.painted_from_cache = true;
        }
        vec![self.seed_now()]
    }

    fn type_char(&mut self, ch: char) -> Vec<Effect> {
        match &mut self.mode {
            Mode::Adding { text } | Mode::Filtering { text } => {
                text.push(ch);
                self.filter_typed()
            }
            Mode::Editing { text, error, .. } => {
                text.push(ch);
                *error = None;
                Vec::new()
            }
            Mode::Palette { query, index } => {
                query.push(ch);
                *index = 0;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// `x`: complete the open tasks among the targets in one request, or
    /// reopen them when all are completed.
    fn toggle_complete(&mut self) -> Vec<Effect> {
        let targets = self.targets();
        if targets.is_empty() {
            return Vec::new();
        }
        let open: Vec<String> = targets
            .iter()
            .filter(|task| !task.completed)
            .map(|task| task.id.clone())
            .collect();
        let effect = if open.is_empty() {
            let ids = targets.iter().map(|task| task.id.clone()).collect();
            change(Write::Reopen, ids, TaskChange::Reopen)
        } else {
            change(Write::Complete, open, TaskChange::Complete)
        };
        self.selection.clear();
        vec![effect]
    }

    /// After the filter's text changed, search again.
    fn filter_typed(&mut self) -> Vec<Effect> {
        match &self.mode {
            Mode::Filtering { text } => {
                let text = text.clone();
                self.set_filter(Some(text))
            }
            _ => Vec::new(),
        }
    }

    fn set_filter(&mut self, text: Option<String>) -> Vec<Effect> {
        let text = text.filter(|text| !text.trim().is_empty());
        if text == self.filter {
            return Vec::new();
        }
        self.filter = text;
        self.filter_error = None;
        vec![self.seed_now()]
    }

    fn submit_add(&mut self) -> Vec<Effect> {
        let Mode::Adding { text } = std::mem::replace(&mut self.mode, Mode::Normal) else {
            return Vec::new();
        };
        let title = text.trim();
        if title.is_empty() || self.still_loading() {
            return Vec::new();
        }
        // A list gets the task; a view adds it to the default list with
        // what puts it in the view, as To Do does.
        let (list, importance, due) = match &self.shown {
            Some(Scope::List { id }) => (Some(id.clone()), None, None),
            Some(Scope::Important) => (None, Some(Importance::High), None),
            Some(Scope::Planned) => (
                None,
                None,
                Some(
                    self.clock
                        .today
                        .format(ms_todo_core::DATE_FORMAT)
                        .to_string(),
                ),
            ),
            _ => (None, None, None),
        };
        vec![Effect {
            tag: Tag::Write(Write::Add),
            request: Request::AddTask {
                task: NewTask {
                    title: title.to_owned(),
                    list,
                    due,
                    reminder: None,
                    importance,
                    body: None,
                },
                dry_run: false,
                op_id: None,
                idempotency_key: None,
            },
        }]
    }

    fn pick_copy(&mut self) -> Vec<Effect> {
        let Mode::Picker {
            target,
            candidates,
            index,
        } = std::mem::replace(&mut self.mode, Mode::Normal)
        else {
            return Vec::new();
        };
        let Some(copy) = candidates.get(index) else {
            return Vec::new();
        };
        vec![Effect {
            tag: Tag::Undo,
            request: Request::Undo {
                target: Some(target),
                copy: Some(copy.id.clone()),
                op_id: None,
                idempotency_key: None,
            },
        }]
    }

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
                },
            })
            .collect()
    }

    /// Ask for the current scope's seed now, superseding any in flight.
    fn seed_now(&mut self) -> Effect {
        self.seeds.latest += 1;
        self.seeds.in_flight = true;
        self.seeds.again = false;
        Effect {
            tag: Tag::Seed(self.seeds.latest),
            request: Request::Seed {
                scope: self.wanted.clone(),
                search: self.filter.as_deref().map(search_query),
            },
        }
    }

    /// The cache changed: read it again, once the seed in flight lands.
    fn reseed(&mut self) -> Vec<Effect> {
        if self.seeds.in_flight {
            self.seeds.again = true;
            Vec::new()
        } else {
            vec![self.seed_now()]
        }
    }

    fn answered(&mut self, tag: Tag, result: Result<ResponseData, ErrorPayload>) -> Vec<Effect> {
        match (tag, result) {
            (Tag::Seed(number), result) => {
                if number != self.seeds.latest {
                    return Vec::new();
                }
                self.seeds.in_flight = false;
                let mut effects = match result {
                    Ok(ResponseData::Seed(seed)) => {
                        let first = !self.seeded;
                        self.apply_seed(seed);
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
                if let Some(scope) = seed.scope {
                    let tasks = seed.tasks.iter().filter_map(Task::from_entity).collect();
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
            (Tag::Write(write), Ok(ResponseData::Applied(applied))) => {
                self.apply_write(write, &applied.items);
                Vec::new()
            }
            (Tag::Undo, Ok(ResponseData::Applied(_))) => {
                self.show(Level::Info, "Undone");
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
        if self.filter.is_some() && error.kind == "invalid_input" {
            // Mid-typing, like an unclosed quote: keep the last results.
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
        let keep = self.selected().map(|task| task.id.clone());
        self.lists = seed
            .lists
            .iter()
            .filter_map(|list| {
                let id = list.get("id").and_then(Value::as_str)?;
                let name = list.get("displayName").and_then(Value::as_str)?;
                Some((id.to_owned(), name.to_owned()))
            })
            .collect();
        self.counts = seed.counts;
        self.activity = seed.activity;
        self.outbox = seed.outbox;
        self.lists_ready = seed.lists_sync.state == SyncState::Ready;
        self.tasks_ready = seed.sync.state == SyncState::Ready;
        self.seeded = true;
        if let Some(scope) = &seed.scope {
            self.wanted = Some(scope.clone());
            if let Some(index) = self.entries().iter().position(|e| e.scope() == *scope) {
                self.sidebar_index = index;
            }
        }
        let changed_scope = self.shown != seed.scope;
        if changed_scope {
            self.selection.clear();
        }
        self.shown = seed.scope;
        self.tasks = seed.tasks.iter().filter_map(Task::from_entity).collect();
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
        for task in items.iter().filter_map(Task::from_entity) {
            let at = self.tasks.iter().position(|row| row.id == task.id);
            match (write, at) {
                (Write::Delete, Some(at)) => {
                    self.tasks.remove(at);
                }
                (Write::Delete, None) => {}
                // A filtered list is the search's to decide.
                (Write::Add, _) if self.filter.is_some() => {}
                (Write::Add, _) if belongs(&shown, &task) => {
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
                (_, Some(at)) if belongs(&shown, &task) => self.tasks[at] = task,
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

    fn event(&mut self, event: Event) -> Vec<Effect> {
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
            Event::SyncProgress(_) | Event::Unknown => Vec::new(),
        }
    }

    pub fn show(&mut self, level: Level, text: &str) {
        self.banner = Some(Banner {
            text: text.to_owned(),
            level,
            ticks_left: BANNER_TICKS,
        });
    }
}

fn change(write: Write, ids: Vec<String>, change: TaskChange) -> Effect {
    Effect {
        tag: Tag::Write(write),
        request: Request::ChangeTasks {
            tasks: ids,
            list: None,
            change,
            dry_run: false,
            op_id: None,
            idempotency_key: None,
        },
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

#[cfg(test)]
pub(crate) mod tests;
