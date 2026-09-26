//! What the TUI is doing now: browsing, or one of its prompts, pickers
//! and pages.

use ms_todo_protocol::{Candidate, TaskChange};

use super::edit::Field;
use super::line_editor::LineEditor;
use super::steps;

#[derive(Clone, Debug, PartialEq)]
pub enum Mode {
    Normal,
    /// `a`: typing a new task. `parsed` is the reading of `input`, redone
    /// on every key; `None` when `Ctrl-r` takes the text literally.
    Adding {
        input: LineEditor,
        parsed: Option<ms_todo_nlp::ParsedTask>,
    },
    /// Typing a filter; the list follows each key.
    Filtering {
        input: LineEditor,
    },
    /// `e`: which field of the task `id` to edit.
    ChoosingField {
        id: String,
    },
    /// Which importance to give the task `id`.
    ChoosingImportance {
        id: String,
    },
    /// Typing a new value for one field of a task.
    Editing {
        id: String,
        field: Field,
        input: LineEditor,
        /// Why the text can't be sent, shown under it.
        error: Option<String>,
    },
    /// Typing a step's text or the link's URL for the task `id`.
    EditingChild {
        id: String,
        target: steps::ChildTarget,
        input: LineEditor,
        /// Why the text can't be sent, shown under it.
        error: Option<String>,
    },
    /// Typing the path of a file to attach to the task `id`.
    Attaching {
        id: String,
        input: LineEditor,
        /// Why the path can't be read, shown under it.
        error: Option<String>,
    },
    /// The inline "Delete …? y/n" for a step or the link of the task `id`,
    /// and the change that deletes it.
    ConfirmDeleteChild {
        id: String,
        change: TaskChange,
        /// What's deleted, as the question names it.
        what: String,
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
        query: LineEditor,
        index: usize,
    },
    Diagnostics,
    /// Typing the folder to move the list `list_id` into; empty takes it
    /// out of its folder.
    MovingList {
        list_id: String,
        input: LineEditor,
    },
    /// Typing a new list's name, or with `list_id`, a list's new name.
    NamingList {
        list_id: Option<String>,
        input: LineEditor,
    },
    /// The inline "Delete …? y/n" for the list `list_id`.
    ConfirmDeleteList {
        list_id: String,
        /// What's deleted, as the question names it.
        what: String,
    },
    /// Typing one due date for the tasks `ids`: `what` names them, as
    /// `"Call Sam"`, `3 tasks` or `2 overdue tasks`.
    SettingDue {
        ids: Vec<String>,
        what: String,
        input: LineEditor,
        /// Why the text can't be sent, shown after it.
        error: Option<String>,
    },
    /// `W`: who the tasks `ids` wait on; empty clears it. `what` names
    /// them, as `"Call Sam"` or `3 tasks`.
    Assigning {
        ids: Vec<String>,
        what: String,
        input: LineEditor,
    },
    /// `m`: which list to move the tasks `ids` to, by typing part of its
    /// name or its folder's; `what` names the tasks, as `"Call Sam"` or
    /// `3 tasks`.
    MovingTasks {
        ids: Vec<String>,
        what: String,
        query: LineEditor,
        index: usize,
    },
    /// Undoing a recurring completion: which completed copy to delete.
    Picker {
        target: String,
        candidates: Vec<Candidate>,
        index: usize,
    },
    /// The help screen, scrolled down `scroll` rows.
    Help {
        scroll: u16,
    },
    /// The theme picker: the theme under the cursor is drawn as a
    /// preview; `before` is put back on Esc.
    Themes {
        index: usize,
        before: &'static crate::theme::Builtin,
    },
    /// A task's links, to open or copy one.
    Links {
        links: Vec<ms_todo_core::links::Link>,
        index: usize,
    },
}
