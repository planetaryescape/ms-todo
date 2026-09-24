/// What a key asks for. The keybinding registry maps keys to these, and
/// `App::update` is the only place they take effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    MoveDown,
    MoveUp,
    JumpTop,
    JumpBottom,
    /// `h`: the pane to the left.
    FocusLeft,
    /// `l`, and Enter in the sidebar: the pane to the right.
    FocusRight,
    /// Tab: the next pane, round to the first.
    FocusNext,
    /// Add a task to the current list; the text is taken literally.
    Add,
    /// Complete an open task, or reopen a completed one.
    ToggleComplete,
    /// Delete, after an inline confirmation.
    Delete,
    Undo,
    /// Filter the current scope by search (D-041).
    Filter,
    /// Esc in the list: drop the selection, else the filter.
    Clear,
    /// `e`, and Enter in the detail pane: edit the field under the detail
    /// pane's cursor.
    Edit,
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
}
