//! The symbols the TUI draws with: Unicode symbols by default, plain
//! ASCII with `--ascii`. Never emoji (docs/blueprint/08-tui.md#layout):
//! every Unicode symbol here has text presentation by default.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Glyphs {
    pub open: &'static str,
    pub done: &'static str,
    pub important: &'static str,
    pub recurring: &'static str,
    pub reminder: &'static str,
    pub pending: &'static str,
    pub unknown: &'static str,
    pub failed: &'static str,
    pub my_day: &'static str,
    pub planned: &'static str,
    pub all: &'static str,
    pub completed: &'static str,
    pub list: &'static str,
    /// A folder's heading in the sidebar, expanded and collapsed.
    pub folder_open: &'static str,
    pub folder_closed: &'static str,
    pub connected: &'static str,
    pub disconnected: &'static str,
    pub cursor: &'static str,
    /// The line editor's Left and Right, in the hint bar.
    pub left_right: &'static str,
    /// Marks a task `v` selected.
    pub selected: &'static str,
    /// A task with attachments, and each attachment in the detail pane.
    pub attachment: &'static str,
    /// Help's scrollbar thumb, and what it says when rows are past the
    /// bottom or the top of its box.
    pub scroll_thumb: &'static str,
    pub more_below: &'static str,
    pub more_above: &'static str,
    /// The Assigned view, and the person chip on a task someone's
    /// assigned.
    pub assigned: &'static str,
}

pub const UNICODE: Glyphs = Glyphs {
    open: "\u{25cb}",      // ○
    done: "\u{2713}",      // ✓
    important: "\u{2605}", // ★
    recurring: "\u{21bb}", // ↻
    reminder: "\u{25f7}",  // ◷
    pending: "\u{25cc}",   // ◌
    unknown: "?",
    failed: "\u{2717}",              // ✗
    my_day: "\u{263c}",              // ☼
    planned: "\u{25a6}",             // ▦
    all: "\u{221e}",                 // ∞
    completed: "\u{2713}",           // ✓
    list: "\u{2261}",                // ≡
    folder_open: "\u{25be}",         // ▾
    folder_closed: "\u{25b8}",       // ▸
    connected: "\u{25cf}",           // ●
    disconnected: "\u{25cb}",        // ○
    cursor: "\u{2588}",              // █
    left_right: "\u{2190}/\u{2192}", // ←/→
    selected: "\u{25c6}",            // ◆
    attachment: "\u{2398}",          // ⎘
    scroll_thumb: "\u{2503}",        // ┃
    more_below: "\u{2193} more",     // ↓
    more_above: "\u{2191} more",     // ↑
    assigned: "\u{25d4}",            // ◔
};

pub const ASCII: Glyphs = Glyphs {
    open: "[ ]",
    done: "[x]",
    important: "!",
    recurring: "R",
    reminder: "@",
    pending: "~",
    unknown: "?",
    failed: "X",
    my_day: "o",
    planned: "#",
    all: "*",
    completed: "x",
    list: "-",
    folder_open: "v",
    folder_closed: ">",
    connected: "+",
    disconnected: "-",
    cursor: "_",
    left_right: "Left/Right",
    selected: "*",
    attachment: "&",
    scroll_thumb: "#",
    more_below: "v more",
    more_above: "^ more",
    assigned: "w",
};
