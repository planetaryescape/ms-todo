use crate::app::edit::Field;

/// What a key asks for. The keybinding registry maps keys to these, and
/// `App::update` is the only place they take effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    MoveDown,
    MoveUp,
    JumpTop,
    JumpBottom,
    /// Enter or Space in the sidebar: collapse or expand a folder, or
    /// open a list or view.
    Open,
    /// "Move list to folder…": ask for a folder for the current list.
    MoveToFolder,
    /// Tab in the folder prompt: take the first suggestion; in quick add,
    /// finish a `#List` or `@label`.
    Complete,
    /// `h`: the pane to the left.
    FocusLeft,
    /// `l`, and Enter in the sidebar: the pane to the right.
    FocusRight,
    /// Tab: the next pane, round to the first.
    FocusNext,
    /// Quick add: a task typed the way it's said, read as it's typed.
    Add,
    /// `Ctrl-r` in quick add: take the text literally, or read it again.
    ToggleParse,
    /// `Ctrl-l` in quick add: type the suggested list in as `#List`.
    AcceptList,
    /// Complete an open task, or reopen a completed one.
    ToggleComplete,
    /// Delete, after an inline confirmation.
    Delete,
    Undo,
    /// Filter the current scope by search (D-041).
    Filter,
    /// Esc in the list: drop the selection, else the filter.
    Clear,
    /// `e`: pick which field of the task to edit.
    Edit,
    /// Enter in the detail pane: edit the field under its cursor.
    EditHere,
    /// A field picked, in the picker or the palette. Importance asks for
    /// a level; the others open their editor.
    EditField(Field),
    /// `I` in the picker: low, normal, high, low, saved at once.
    CycleImportance,
    /// A level typed in the importance picker, as `ms_todo_nlp`'s
    /// `read_importance` reads it: `1`–`4`, `high`, `normal`, `low`.
    SetImportance(&'static str),
    /// `S`: one due date for the selection, or the task under the cursor.
    SetDue,
    /// `R`: a new due date for every overdue open task on screen.
    RescheduleOverdue,
    /// `m`: move the selection, or the task under the cursor, to a list
    /// picked by name.
    MoveTasks,
    /// `v`: add the task to the selection, or take it out.
    ToggleSelect,
    /// `V`: select every task in the view.
    SelectAll,
    /// `:`: the command palette.
    Palette,
    /// `D`: the diagnostics page.
    Diagnostics,
    /// `r` on the diagnostics page: ask the daemon again.
    Refresh,
    Sync,
    Help,
    Quit,
    /// Enter in a prompt or the picker.
    Submit,
    /// Esc in a prompt, the confirmation, the picker or help.
    Cancel,
    /// `y` in the delete confirmation.
    Confirm,
    Backspace,
    /// Enter in the notes editor.
    Newline,
    /// `o`: open the task's link, or pick one of its links.
    OpenLink,
    /// `y`: copy the task's link, or pick one of its links to copy.
    CopyLink,
}
