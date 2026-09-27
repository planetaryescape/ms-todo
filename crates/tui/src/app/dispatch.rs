//! Where each message and action goes: the mode decides first, then
//! browsing takes the rest.

use ms_todo_protocol::{Request, TaskChange};

use super::line_editor::LineEditor;
use super::{App, Connection, Effect, Level, Mode, Msg, Pane, Tag, Write, change};
use crate::action::Action;

impl App {
    pub(super) fn handle(&mut self, msg: Msg) -> Vec<Effect> {
        match msg {
            Msg::Action(action)
                if self.sign_in_required
                    && self.mode == Mode::Normal
                    && !matches!(action, Action::Quit | Action::Help | Action::Diagnostics) =>
            {
                Vec::new()
            }
            Msg::Action(action) => self.act(action),
            Msg::Char(ch) => self.edit_with(|input| {
                input.insert(ch);
                true
            }),
            Msg::Key(key) => self.edit_with(|input| input.key(key)),
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
            Msg::Resize(size) => {
                self.screen = size;
                if let Mode::Help { scroll } = &mut self.mode {
                    *scroll = (*scroll).min(crate::help::layout(size).max_scroll());
                }
                Vec::new()
            }
            Msg::Response { tag, result } => self.answered(tag, result),
            Msg::Event(event) => self.event(event),
            Msg::Tick(clock) => {
                let new_day = clock.today() != self.clock.today();
                self.clock = clock;
                self.reread_quick_add();
                if let Some(banner) = &mut self.banner {
                    banner.ticks_left = banner.ticks_left.saturating_sub(1);
                    if banner.ticks_left == 0 {
                        self.banner = None;
                    }
                }
                let mut effects = self.tick_list_hint();
                // A deferred task's day may have come, and Next, Upcoming
                // and the counts move with the date: read the views again.
                if new_day && self.seeded {
                    self.cache.clear();
                    effects.extend(self.reseed());
                }
                effects
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
            (Mode::Themes { .. }, Action::MoveDown | Action::MoveUp) => {
                self.step_theme(action == Action::MoveDown);
                Vec::new()
            }
            (Mode::Themes { .. }, Action::Submit) => {
                self.keep_theme();
                Vec::new()
            }
            (Mode::Links { index, links }, Action::MoveDown) => {
                *index = (*index + 1).min(links.len().saturating_sub(1));
                Vec::new()
            }
            (Mode::Links { index, .. }, Action::MoveUp) => {
                *index = index.saturating_sub(1);
                Vec::new()
            }
            (Mode::Links { .. }, Action::Submit | Action::OpenLink | Action::CopyLink) => {
                self.pick_link(action == Action::CopyLink);
                Vec::new()
            }
            (Mode::Triage(_), Action::Submit) => self.triage_move(),
            (Mode::Triage(_), Action::Skip) => self.triage_skip(),
            (Mode::Triage(_), Action::Cancel) => self.stop_triage(),
            (Mode::Themes { .. }, Action::Cancel) => {
                self.revert_theme();
                Vec::new()
            }
            (Mode::MovingTasks { .. }, Action::MoveDown | Action::MoveUp) => {
                self.move_step(action == Action::MoveDown);
                Vec::new()
            }
            (Mode::MovingTasks { .. }, Action::Submit) => self.submit_move_tasks(),
            (_, Action::Backspace) => self.edit_with(LineEditor::backspace),
            (Mode::Editing { .. }, Action::Newline) => self.edit_with(LineEditor::newline),
            (
                Mode::Help { scroll },
                Action::MoveDown
                | Action::MoveUp
                | Action::PageDown
                | Action::PageUp
                | Action::JumpTop
                | Action::JumpBottom,
            ) => {
                // The layout the frame is drawn from, so j stops at the last row.
                let page = crate::help::layout(self.screen);
                *scroll = match action {
                    Action::MoveDown => scroll.saturating_add(1),
                    Action::MoveUp => scroll.saturating_sub(1),
                    Action::PageDown => scroll.saturating_add(page.page()),
                    Action::PageUp => scroll.saturating_sub(page.page()),
                    Action::JumpTop => 0,
                    _ => u16::MAX,
                }
                .min(page.max_scroll());
                Vec::new()
            }
            (Mode::Diagnostics, Action::MoveDown | Action::MoveUp) => {
                self.scroll_diagnostics(action == Action::MoveDown);
                Vec::new()
            }
            (Mode::Diagnostics, Action::Refresh) => self.refresh_diagnostics(),
            (Mode::Editing { .. }, Action::Submit) => self.submit_edit(),
            (Mode::EditingChild { .. }, Action::Submit) => self.submit_child(),
            (Mode::EditingChild { .. }, Action::Complete) => {
                self.next_link_part();
                Vec::new()
            }
            (Mode::Attaching { .. }, Action::Submit) => self.submit_attach(),
            (Mode::ConfirmDeleteChild { .. }, Action::Confirm) => self.confirm_child_delete(),
            (Mode::SettingDue { .. }, Action::Submit) => self.submit_set_due(),
            (Mode::ConfirmClearDue { .. }, Action::Confirm) => self.confirm_clear_due(),
            (Mode::Assigning { .. }, Action::Submit) => self.submit_assign(),
            (Mode::NamingList { .. }, Action::Submit) => self.submit_list_name(),
            (Mode::ConfirmDeleteList { .. }, Action::Confirm) => self.confirm_delete_list(),
            (Mode::ChoosingField { .. }, Action::EditField(_) | Action::CycleImportance)
            | (Mode::ChoosingImportance { .. }, Action::SetImportance(_)) => {
                self.edit_action(action)
            }
            (Mode::Adding { .. }, Action::Submit) => self.submit_add(),
            (Mode::Adding { .. }, Action::Complete) => {
                self.complete_quick_add();
                Vec::new()
            }
            (Mode::Adding { .. }, Action::ToggleParse) => {
                self.toggle_parse();
                self.quick_add_typed();
                Vec::new()
            }
            (Mode::Adding { .. }, Action::AcceptList) => {
                self.accept_list_hint();
                Vec::new()
            }
            (Mode::MovingList { .. }, Action::Submit) => self.submit_move(),
            (Mode::MovingList { .. }, Action::Complete) => {
                self.complete_folder();
                Vec::new()
            }
            (Mode::Filtering { .. }, Action::Submit) => {
                self.mode = Mode::Normal;
                self.focus = Pane::Tasks;
                Vec::new()
            }
            (Mode::Filtering { .. }, Action::Cancel) => {
                self.mode = Mode::Normal;
                self.set_filter(None)
            }
            (Mode::Filtering { .. }, Action::ToggleSemantic) => self.toggle_semantic(),
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

    pub(super) fn browse(&mut self, action: Action) -> Vec<Effect> {
        if self.on_child_row()
            && !self.still_loading()
            && let Some(effects) = self.child_action(action)
        {
            return effects;
        }
        match action {
            Action::MoveDown
            | Action::MoveUp
            | Action::PageDown
            | Action::PageUp
            | Action::JumpTop
            | Action::JumpBottom => self.navigate(action),
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
                    self.start_add()
                } else {
                    self.show(
                        Level::Info,
                        "Still syncing your lists; try again in a moment",
                    );
                    Vec::new()
                }
            }
            Action::ToggleComplete
            | Action::Delete
            | Action::SetDue
            | Action::RescheduleOverdue
            | Action::MoveTasks
            | Action::ToggleMyDay
            | Action::ToggleNag
            | Action::Attach
            | Action::Assign
                if self.still_loading() =>
            {
                Vec::new()
            }
            Action::SetDue => {
                self.start_set_due();
                Vec::new()
            }
            Action::RescheduleOverdue => {
                self.start_reschedule_overdue();
                Vec::new()
            }
            Action::ToggleComplete => self.toggle_complete(),
            Action::Attach => {
                if let Some(task) = self.selected().cloned() {
                    self.start_attach(&task);
                }
                Vec::new()
            }
            Action::ToggleMyDay => self.toggle_my_day(),
            Action::ToggleNag => self.toggle_nag(),
            Action::Assign => {
                self.start_assign();
                Vec::new()
            }
            Action::MoveTasks => {
                self.start_move_tasks();
                Vec::new()
            }
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
            Action::Edit | Action::EditHere | Action::EditField(_) | Action::CycleImportance => {
                self.edit_action(action)
            }
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
            Action::OpenLink | Action::CopyLink => {
                self.follow_link(action == Action::CopyLink);
                Vec::new()
            }
            Action::Open => self.open_row(),
            Action::MoveToFolder => {
                self.start_move();
                Vec::new()
            }
            Action::ReorderUp | Action::ReorderDown => self.reorder(action == Action::ReorderDown),
            Action::NewList => {
                self.start_new_list();
                Vec::new()
            }
            Action::RenameList => {
                self.start_rename_list();
                Vec::new()
            }
            Action::DeleteList => {
                self.start_delete_list();
                Vec::new()
            }
            Action::Diagnostics => self.open_diagnostics(),
            Action::ToggleDeferred => self.toggle_deferred(),
            Action::NextContext => self.next_context(),
            Action::TriageInbox => self.start_triage(),
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
                    input: LineEditor::single(self.filter.as_deref().unwrap_or_default()),
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
                self.mode = Mode::Help { scroll: 0 };
                Vec::new()
            }
            Action::Quit => {
                self.should_quit = true;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// A key for the prompt's line editor. When the text changes, a
    /// filter searches again, an edit's error goes and the palette goes
    /// back to its best match.
    fn edit_with(&mut self, edit: impl FnOnce(&mut LineEditor) -> bool) -> Vec<Effect> {
        let changed = match &mut self.mode {
            Mode::Adding { input, .. } => {
                let changed = edit(input);
                if changed {
                    self.reread_quick_add();
                    self.quick_add_typed();
                }
                return Vec::new();
            }
            Mode::Filtering { input }
            | Mode::MovingList { input, .. }
            | Mode::NamingList { input, .. }
            | Mode::Assigning { input, .. } => edit(input),
            Mode::Editing { input, error, .. }
            | Mode::EditingChild { input, error, .. }
            | Mode::Attaching { input, error, .. }
            | Mode::SettingDue { input, error, .. } => {
                let changed = edit(input);
                if changed {
                    *error = None;
                }
                changed
            }
            Mode::Palette { query, index } | Mode::MovingTasks { query, index, .. } => {
                let changed = edit(query);
                if changed {
                    *index = 0;
                }
                changed
            }
            _ => false,
        };
        if changed {
            self.filter_typed()
        } else {
            Vec::new()
        }
    }
}
