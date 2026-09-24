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
    pub planned: &'static str,
    pub all: &'static str,
    pub completed: &'static str,
    pub list: &'static str,
    pub connected: &'static str,
    pub disconnected: &'static str,
    pub cursor: &'static str,
    /// Marks a task `v` selected.
    pub selected: &'static str,
}

pub const UNICODE: Glyphs = Glyphs {
    open: "\u{25cb}",      // ○
    done: "\u{2713}",      // ✓
    important: "\u{2605}", // ★
    recurring: "\u{21bb}", // ↻
    reminder: "\u{25f7}",  // ◷
    pending: "\u{25cc}",   // ◌
    unknown: "?",
    failed: "\u{2717}",       // ✗
    planned: "\u{25a6}",      // ▦
    all: "\u{221e}",          // ∞
    completed: "\u{2713}",    // ✓
    list: "\u{2261}",         // ≡
    connected: "\u{25cf}",    // ●
    disconnected: "\u{25cb}", // ○
    cursor: "\u{2588}",       // █
    selected: "\u{25c6}",     // ◆
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
    planned: "#",
    all: "*",
    completed: "x",
    list: "-",
    connected: "+",
    disconnected: "-",
    cursor: "_",
    selected: "*",
};
