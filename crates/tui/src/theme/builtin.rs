//! The built-in themes: `terminal`, which follows the terminal's own
//! colours, then a few well-known palettes, each from its official source.
//! Restful by design: borders and secondary text recede, the selection is
//! a quiet background, and colour is kept for what means something
//! (overdue, important, sync state, errors).

use ratatui::style::Color;

use super::{Builtin, Palette};

/// The theme when neither `--theme` nor `[tui] theme` names one.
pub const DEFAULT: &str = "terminal";

const fn hex(rgb: u32) -> Color {
    Color::from_u32(rgb)
}

pub static BUILTIN: [Builtin; 8] = [
    Builtin {
        name: DEFAULT,
        palette: TERMINAL,
    },
    Builtin {
        name: "catppuccin-mocha",
        palette: CATPPUCCIN_MOCHA,
    },
    Builtin {
        name: "catppuccin-latte",
        palette: CATPPUCCIN_LATTE,
    },
    Builtin {
        name: "gruvbox-dark",
        palette: GRUVBOX_DARK,
    },
    Builtin {
        name: "gruvbox-light",
        palette: GRUVBOX_LIGHT,
    },
    Builtin {
        name: "tokyo-night",
        palette: TOKYO_NIGHT,
    },
    Builtin {
        name: "nord",
        palette: NORD,
    },
    Builtin {
        name: "high-contrast",
        palette: HIGH_CONTRAST,
    },
];

/// The terminal's own palette: its background and text, its ANSI colours,
/// and dim for secondary text, so a Ghostty or iTerm theme carries over.
/// Bright black (`DarkGray`) is the one grey every terminal theme defines.
const TERMINAL: Palette = Palette {
    background: Color::Reset,
    text: Color::Reset,
    text_dim: Color::Reset,
    text_muted: Color::Reset,
    border: Color::DarkGray,
    border_focused: Color::Cyan,
    title: Color::Cyan,
    selection_bg: Color::DarkGray,
    selection_fg: Color::Reset,
    accent: Color::Cyan,
    overdue: Color::Red,
    due_today: Color::Cyan,
    important: Color::Yellow,
    completed: Color::Reset,
    sync_pending: Color::Reset,
    sync_unknown: Color::Yellow,
    sync_failed: Color::Red,
    error: Color::Red,
    warning: Color::Yellow,
    banner_error: Color::Red,
    banner_info: Color::Cyan,
    search_match: Color::Yellow,
    cursor: Color::Reset,
    header_bar: Color::Reset,
};

// Catppuccin, https://github.com/catppuccin/palette/blob/main/palette.json,
// used as its style guide says
// (https://github.com/catppuccin/catppuccin/blob/main/docs/style-guide.md):
// base behind, surface for the selection, overlay for quiet text,
// lavender for the active border, rosewater for the cursor.
const CATPPUCCIN_MOCHA: Palette = Palette {
    background: hex(0x1e1e2e),     // base
    text: hex(0xcdd6f4),           // text
    text_dim: hex(0xa6adc8),       // subtext0
    text_muted: hex(0x7f849c),     // overlay1
    border: hex(0x45475a),         // surface1
    border_focused: hex(0xb4befe), // lavender
    title: hex(0xb4befe),          // lavender
    selection_bg: hex(0x313244),   // surface0
    selection_fg: Color::Reset,
    accent: hex(0x89b4fa),       // blue
    overdue: hex(0xf38ba8),      // red
    due_today: hex(0xfab387),    // peach
    important: hex(0xf9e2af),    // yellow
    completed: hex(0x6c7086),    // overlay0
    sync_pending: hex(0x7f849c), // overlay1
    sync_unknown: hex(0xf9e2af), // yellow
    sync_failed: hex(0xf38ba8),  // red
    error: hex(0xf38ba8),        // red
    warning: hex(0xf9e2af),      // yellow
    banner_error: hex(0xf38ba8), // red
    banner_info: hex(0x94e2d5),  // teal
    search_match: hex(0xcba6f7), // mauve
    cursor: hex(0xf5e0dc),       // rosewater
    header_bar: hex(0x181825),   // mantle
};

// The same roles from Catppuccin Latte, the light flavour (source above).
const CATPPUCCIN_LATTE: Palette = Palette {
    background: hex(0xeff1f5),     // base
    text: hex(0x4c4f69),           // text
    text_dim: hex(0x6c6f85),       // subtext0
    text_muted: hex(0x8c8fa1),     // overlay1
    border: hex(0xbcc0cc),         // surface1
    border_focused: hex(0x7287fd), // lavender
    title: hex(0x7287fd),          // lavender
    selection_bg: hex(0xccd0da),   // surface0
    selection_fg: Color::Reset,
    accent: hex(0x1e66f5),       // blue
    overdue: hex(0xd20f39),      // red
    due_today: hex(0xfe640b),    // peach
    important: hex(0xdf8e1d),    // yellow
    completed: hex(0x9ca0b0),    // overlay0
    sync_pending: hex(0x8c8fa1), // overlay1
    sync_unknown: hex(0xdf8e1d), // yellow
    sync_failed: hex(0xd20f39),  // red
    error: hex(0xd20f39),        // red
    warning: hex(0xdf8e1d),      // yellow
    banner_error: hex(0xd20f39), // red
    banner_info: hex(0x179299),  // teal
    search_match: hex(0x8839ef), // mauve
    cursor: hex(0xdc8a78),       // rosewater
    header_bar: hex(0xe6e9ef),   // mantle
};

// Gruvbox, https://github.com/morhetz/gruvbox (the palette in its README
// and colors/gruvbox.vim): the dark mode's bright accents.
const GRUVBOX_DARK: Palette = Palette {
    background: hex(0x282828),     // dark0
    text: hex(0xebdbb2),           // light1
    text_dim: hex(0xa89984),       // light4
    text_muted: hex(0x928374),     // gray
    border: hex(0x504945),         // dark2
    border_focused: hex(0x83a598), // bright_blue
    title: hex(0xfabd2f),          // bright_yellow
    selection_bg: hex(0x3c3836),   // dark1
    selection_fg: Color::Reset,
    accent: hex(0x83a598),       // bright_blue
    overdue: hex(0xfb4934),      // bright_red
    due_today: hex(0xfe8019),    // bright_orange
    important: hex(0xfabd2f),    // bright_yellow
    completed: hex(0x7c6f64),    // dark4
    sync_pending: hex(0x928374), // gray
    sync_unknown: hex(0xfabd2f), // bright_yellow
    sync_failed: hex(0xfb4934),  // bright_red
    error: hex(0xfb4934),        // bright_red
    warning: hex(0xfe8019),      // bright_orange
    banner_error: hex(0xfb4934), // bright_red
    banner_info: hex(0x8ec07c),  // bright_aqua
    search_match: hex(0xd3869b), // bright_purple
    cursor: hex(0xebdbb2),       // light1
    header_bar: hex(0x1d2021),   // dark0_hard
};

// Gruvbox's light mode (source above): its faded accents, which keep
// their contrast on the cream background.
const GRUVBOX_LIGHT: Palette = Palette {
    background: hex(0xfbf1c7),     // light0
    text: hex(0x3c3836),           // dark1
    text_dim: hex(0x7c6f64),       // dark4
    text_muted: hex(0x928374),     // gray
    border: hex(0xd5c4a1),         // light2
    border_focused: hex(0x076678), // faded_blue
    title: hex(0xb57614),          // faded_yellow
    selection_bg: hex(0xebdbb2),   // light1
    selection_fg: Color::Reset,
    accent: hex(0x076678),       // faded_blue
    overdue: hex(0x9d0006),      // faded_red
    due_today: hex(0xaf3a03),    // faded_orange
    important: hex(0xb57614),    // faded_yellow
    completed: hex(0xa89984),    // light4
    sync_pending: hex(0x928374), // gray
    sync_unknown: hex(0xb57614), // faded_yellow
    sync_failed: hex(0x9d0006),  // faded_red
    error: hex(0x9d0006),        // faded_red
    warning: hex(0xaf3a03),      // faded_orange
    banner_error: hex(0x9d0006), // faded_red
    banner_info: hex(0x427b58),  // faded_aqua
    search_match: hex(0x8f3f71), // faded_purple
    cursor: hex(0x3c3836),       // dark1
    header_bar: hex(0xf9f5d7),   // light0_hard
};

// Tokyo Night's "night" style,
// https://github.com/folke/tokyonight.nvim/blob/main/extras/lua/tokyonight_night.lua.
const TOKYO_NIGHT: Palette = Palette {
    background: hex(0x1a1b26),     // bg
    text: hex(0xc0caf5),           // fg
    text_dim: hex(0xa9b1d6),       // fg_dark
    text_muted: hex(0x737aa2),     // dark5
    border: hex(0x3b4261),         // fg_gutter
    border_focused: hex(0x7aa2f7), // blue
    title: hex(0xbb9af7),          // magenta
    selection_bg: hex(0x292e42),   // bg_highlight
    selection_fg: Color::Reset,
    accent: hex(0x7aa2f7),       // blue
    overdue: hex(0xf7768e),      // red
    due_today: hex(0xff9e64),    // orange
    important: hex(0xe0af68),    // yellow
    completed: hex(0x565f89),    // comment
    sync_pending: hex(0x737aa2), // dark5
    sync_unknown: hex(0xe0af68), // yellow
    sync_failed: hex(0xf7768e),  // red
    error: hex(0xf7768e),        // red
    warning: hex(0xff9e64),      // orange
    banner_error: hex(0xf7768e), // red
    banner_info: hex(0x7dcfff),  // cyan
    search_match: hex(0x9ece6a), // green
    cursor: hex(0xc0caf5),       // fg
    header_bar: hex(0x16161e),   // bg_dark
};

// Nord, https://www.nordtheme.com/docs/colors-and-palettes. Nord3 is too
// dark for text on Nord0, so quiet text is the brighter comment grey of
// Nord's own Vim port (nord3_gui_bright,
// https://github.com/nordtheme/vim/blob/main/colors/nord.vim).
const NORD: Palette = Palette {
    background: hex(0x2e3440),     // nord0
    text: hex(0xeceff4),           // nord6
    text_dim: hex(0xd8dee9),       // nord4
    text_muted: hex(0x616e88),     // nord3_gui_bright
    border: hex(0x4c566a),         // nord3
    border_focused: hex(0x88c0d0), // nord8
    title: hex(0x88c0d0),          // nord8
    selection_bg: hex(0x3b4252),   // nord1
    selection_fg: Color::Reset,
    accent: hex(0x88c0d0),       // nord8
    overdue: hex(0xbf616a),      // nord11
    due_today: hex(0xd08770),    // nord12
    important: hex(0xebcb8b),    // nord13
    completed: hex(0x616e88),    // nord3_gui_bright
    sync_pending: hex(0x616e88), // nord3_gui_bright
    sync_unknown: hex(0xebcb8b), // nord13
    sync_failed: hex(0xbf616a),  // nord11
    error: hex(0xbf616a),        // nord11
    warning: hex(0xd08770),      // nord12
    banner_error: hex(0xbf616a), // nord11
    banner_info: hex(0x8fbcbb),  // nord7
    search_match: hex(0xb48ead), // nord15
    cursor: hex(0xd8dee9),       // nord4
    header_bar: hex(0x3b4252),   // nord1
};

// No official palette exists for this one: every text colour meets
// WCAG 2.2's enhanced contrast, 7:1 against the black background
// (https://www.w3.org/WAI/WCAG22/Understanding/contrast-enhanced.html),
// which the `high_contrast_meets_wcag_aaa` test checks.
const HIGH_CONTRAST: Palette = Palette {
    background: hex(0x000000),
    text: hex(0xffffff),
    text_dim: hex(0xd0d0d0),
    text_muted: hex(0xb8b8b8),
    border: hex(0xb8b8b8),
    border_focused: hex(0xffff00),
    title: hex(0xffff00),
    selection_bg: hex(0x1f3a93),
    selection_fg: hex(0xffffff),
    accent: hex(0x00ffff),
    overdue: hex(0xff6e6e),
    due_today: hex(0xffb86c),
    important: hex(0xffff00),
    completed: hex(0xb8b8b8),
    sync_pending: hex(0xd0d0d0),
    sync_unknown: hex(0xffff00),
    sync_failed: hex(0xff6e6e),
    error: hex(0xff6e6e),
    warning: hex(0xffb86c),
    banner_error: hex(0xff6e6e),
    banner_info: hex(0x00ffff),
    search_match: hex(0xff80ff),
    cursor: hex(0xffffff),
    header_bar: hex(0x000000),
};
