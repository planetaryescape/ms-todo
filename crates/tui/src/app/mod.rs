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

mod answers;
mod assign;
pub mod attachments;
mod detail_cursor;
pub mod diagnostics;
mod dispatch;
mod due_batch;
pub mod edit;
mod filter;
pub(crate) mod folders;
pub mod line_editor;
mod links;
pub mod list_hint;
mod list_writes;
mod messages;
mod mode;
pub mod move_tasks;
pub(crate) mod my_day;
mod navigation;
pub mod palette;
pub mod quick_add;
pub mod scope;
mod selection;
pub mod steps;
pub mod task;
mod task_writes;
mod themes;

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, FixedOffset, Local, NaiveDate};
use ms_todo_protocol::{Counts, OutboxDepth, Request, Scope, SyncActivity, TaskChange};
use ratatui::layout::Size;

use crate::glyphs::Glyphs;
use crate::keybindings::Context;
use crate::theme::{Theme, ThemeChoice};
use diagnostics::Diagnostics;
use edit::Field;
pub use filter::search_query;
use line_editor::LineEditor;
pub use messages::{Effect, LocalEffect, Msg, Tag, Write};
pub use mode::Mode;
use scope::{Entry, SidebarList, sidebar_entries};
pub use task::{SyncMarker, Task};

/// How long a banner stays, in ticks (the runner ticks every 250 ms).
pub const BANNER_TICKS: u32 = 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Sidebar,
    Tasks,
    Detail,
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

/// Now, for what's drawn relative to it ("synced 5s ago", Overdue) and
/// what typed dates mean ("tomorrow"). Set by ticks, so tests pick their
/// own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clock {
    /// In the local zone.
    pub now: DateTime<FixedOffset>,
}

impl Clock {
    pub fn now() -> Self {
        Self {
            now: Local::now().fixed_offset(),
        }
    }

    pub fn unix(&self) -> i64 {
        self.now.timestamp()
    }

    pub fn today(&self) -> NaiveDate {
        self.now.date_naive()
    }
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
    /// The styles every frame is drawn with, from `theme_choice`.
    pub theme: Theme,
    pub theme_choice: ThemeChoice,
    /// Something for the runner to do on this machine after the frame:
    /// I/O, which `update` doesn't do.
    pub local: Option<LocalEffect>,
    /// ms-todo's version, as the title bar and help show it.
    pub version: &'static str,
    pub focus: Pane,
    pub mode: Mode,
    pub connection: Connection,
    /// The command for this instance while no credential is stored.
    pub sign_in_command: Option<String>,
    /// The first seed could not read any cached tasks without sign-in.
    pub sign_in_required: bool,
    /// Every list, in the daemon's order: folder by folder, then those in
    /// no folder.
    pub lists: Vec<SidebarList>,
    /// The folders collapsed in the sidebar, by name, for this session.
    pub collapsed: HashSet<String>,
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
    /// The detail pane's cursor: the field `e` and Enter edit, or a step
    /// or the link.
    pub detail_row: steps::DetailRow,
    /// Which step or link `detail_row` is on, by ID, so a refresh can't
    /// slide it onto another.
    detail_anchor: Option<detail_cursor::Anchor>,
    /// The step or link the cursor was on changed in a refresh: the next
    /// step or link action says so and does nothing.
    detail_stale: bool,
    /// The tasks `v` and `V` selected, by ID; all rows of `tasks`.
    pub selection: HashSet<String>,
    pub diagnostics: Diagnostics,
    /// Whether the shown scope has synced once.
    pub tasks_ready: bool,
    /// The applied filter.
    pub filter: Option<String>,
    /// Whether the filter searches by meaning (D-062) rather than words.
    pub semantic_filter: bool,
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
    /// The categories quick add's `@label` knows.
    pub categories: quick_add::Categories,
    /// Quick add's list suggestion.
    pub list_hint: list_hint::ListHint,
    /// My Day's day, as the daemon last said (it turns over at
    /// `my_day.rollover_time`); until then, today.
    pub my_day_date: Option<NaiveDate>,
    /// Where attachments are saved, and how typed paths are read.
    pub places: attachments::Places,
    /// The terminal's size, for what scrolls by the screenful.
    pub screen: Size,
    /// `z`: deferred and Someday tasks shown in every view, for this
    /// session.
    pub show_deferred: bool,
}

impl App {
    pub fn new(glyphs: Glyphs, clock: Clock) -> Self {
        Self {
            glyphs,
            theme: Theme::default(),
            theme_choice: ThemeChoice::default(),
            local: None,
            version: env!("CARGO_PKG_VERSION"),
            focus: Pane::Tasks,
            mode: Mode::Normal,
            connection: Connection::Connecting,
            sign_in_command: None,
            sign_in_required: false,
            lists: Vec::new(),
            collapsed: HashSet::new(),
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
            detail_row: steps::DetailRow::default(),
            detail_anchor: None,
            detail_stale: false,
            selection: HashSet::new(),
            diagnostics: Diagnostics::default(),
            tasks_ready: false,
            filter: None,
            semantic_filter: false,
            filter_error: None,
            banner: None,
            rejections: HashMap::new(),
            clock,
            should_quit: false,
            seeds: Seeds::default(),
            cache: HashMap::new(),
            painted_from_cache: false,
            categories: quick_add::Categories::default(),
            list_hint: list_hint::ListHint::default(),
            my_day_date: None,
            places: attachments::Places::default(),
            screen: Size::new(80, 24),
            show_deferred: false,
        }
    }

    /// With the download directory and where typed paths start.
    pub fn with_places(mut self, places: attachments::Places) -> Self {
        self.places = places;
        self
    }

    pub fn with_sign_in_command(mut self, command: Option<String>) -> Self {
        self.sign_in_command = command;
        self
    }

    /// My Day's day.
    pub fn my_day(&self) -> NaiveDate {
        self.my_day_date.unwrap_or_else(|| self.clock.today())
    }

    /// Where keys go now.
    pub fn context(&self) -> Context {
        match self.mode {
            Mode::Editing {
                field: Field::Notes,
                ..
            } => Context::Notes,
            Mode::Adding { .. } => Context::Adding,
            Mode::Filtering { .. }
            | Mode::Editing { .. }
            | Mode::EditingChild { .. }
            | Mode::Attaching { .. }
            | Mode::SettingDue { .. }
            | Mode::Assigning { .. }
            | Mode::NamingList { .. } => Context::Prompt,
            Mode::ChoosingField { .. } => Context::Fields,
            Mode::MovingList { .. } => Context::Folder,
            Mode::ChoosingImportance { .. } => Context::Importance,
            Mode::ConfirmDelete { .. }
            | Mode::ConfirmDeleteChild { .. }
            | Mode::ConfirmDeleteList { .. } => Context::Confirm,
            Mode::Picker { .. } => Context::Picker,
            Mode::Palette { .. } => Context::Palette,
            Mode::MovingTasks { .. } => Context::MoveTo,
            Mode::Diagnostics => Context::Diagnostics,
            Mode::Help { .. } => Context::Help,
            Mode::Themes { .. } => Context::Themes,
            Mode::Links { .. } => Context::Links,
            Mode::Normal => match self.focus {
                Pane::Sidebar => Context::Sidebar,
                Pane::Tasks => Context::Tasks,
                Pane::Detail if self.on_child_row() => Context::Steps,
                Pane::Detail => Context::Detail,
            },
        }
    }

    /// The sidebar's rows: the smart views, then the folders, each with its
    /// lists unless it's collapsed, then the lists in no folder.
    pub fn entries(&self) -> Vec<Entry> {
        sidebar_entries(&self.lists, &self.collapsed, &self.counts.lists)
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
            .find(|list| list.id == id)
            .map(|list| list.name.as_str())
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
        let scope = self.wanted.as_ref().or(self.shown.as_ref());
        let name = self.scope_name(scope);
        if scope == Some(&Scope::MyDay) {
            // The day, since it turns over at the rollover time.
            return format!("{name} \u{b7} {}", self.my_day().format("%a %-d %b"));
        }
        name
    }

    /// The terminal window's title (08-tui.md): which app, and where. A
    /// list's name comes from Graph and the terminal would act on escape
    /// sequences in it, so control characters are removed.
    pub fn window_title(&self) -> String {
        ms_todo_core::one_line_safe(&format!("ms-todo \u{2014} {}", self.view_name()))
    }

    pub fn update(&mut self, msg: Msg) -> Vec<Effect> {
        let effects = self.handle(msg);
        self.follow_detail_row();
        effects
    }

    pub fn show(&mut self, level: Level, text: &str) {
        self.banner = Some(Banner {
            text: text.to_owned(),
            level,
            ticks_left: BANNER_TICKS,
        });
    }
}

pub(super) fn change(write: Write, ids: Vec<String>, change: TaskChange) -> Effect {
    Effect {
        tag: Tag::Write(write),
        request: Request::ChangeTasks {
            tasks: ids,
            list: None,
            select: None,
            change,
            dry_run: false,
            op_id: None,
            idempotency_key: None,
        },
    }
}

#[cfg(test)]
pub(crate) mod tests;
