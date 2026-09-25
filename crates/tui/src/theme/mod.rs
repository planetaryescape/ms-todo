//! How the TUI looks (D-049): a [`Palette`] names a colour for each
//! semantic role, a built-in theme is one palette, and [`Theme`] turns a
//! palette into the styles the `ui` module draws with. Nothing outside
//! this module names a colour or a modifier (the `no_raw_styles` test),
//! so a theme can't miss a spot.
//!
//! The default, `terminal`, uses the terminal's own palette (ANSI colours,
//! dim and bold), so it follows whatever theme the terminal has. The
//! others are fixed RGB palettes, brought down to the nearest 256-colour
//! index where the terminal doesn't say it has truecolor. `NO_COLOR` wins
//! over all of them: bold, dim and reverse only.

mod builtin;
mod config;

use ratatui::style::{Color, Modifier, Style};

pub use builtin::{BUILTIN, DEFAULT};
pub use config::{ThemeError, load, save};

/// Declares [`Palette`] and [`ROLES`] from one list, so a role can't be in
/// one and not the other, and `[tui.colors]` accepts exactly the fields.
macro_rules! palette {
    ($($(#[$doc:meta])* $role:ident,)*) => {
        /// A colour for every role. `Color::Reset` means the terminal's
        /// own: its background, its text colour, or for the roles
        /// [`Theme`] softens, its text colour dimmed.
        #[derive(Clone, Debug, PartialEq, Eq)]
        pub struct Palette {
            $($(#[$doc])* pub $role: Color,)*
        }

        /// The roles by name, as `[tui.colors]` takes them.
        pub const ROLES: &[&str] = &[$(stringify!($role)),*];

        impl Palette {
            /// The role called `name` in `[tui.colors]`.
            fn role_mut(&mut self, name: &str) -> Option<&mut Color> {
                match name {
                    $(stringify!($role) => Some(&mut self.$role),)*
                    _ => None,
                }
            }
        }
    };
}

palette! {
    /// Behind everything.
    background,
    text,
    /// Secondary text: labels, counts, hints.
    text_dim,
    /// Quieter still: separators, the recurring and reminder flags.
    text_muted,
    border,
    border_focused,
    /// Pane titles, the app's name and group headings.
    title,
    /// The row under the cursor: a background, never an inverted block.
    selection_bg,
    /// The selected row's text; `Reset` keeps each cell's own colour, so
    /// an overdue date stays red under the cursor.
    selection_fg,
    /// Keys in the hints, prompts, "syncing", tasks `v` selected.
    accent,
    overdue,
    due_today,
    important,
    completed,
    sync_pending,
    sync_unknown,
    sync_failed,
    /// Why typed text can't be sent, an unreachable daemon.
    error,
    /// A failed sync, a problem on the diagnostics page.
    warning,
    banner_error,
    banner_info,
    /// The filter's text in the task list's title.
    search_match,
    /// A URL in a task's notes, underlined too.
    link,
    /// The line editor's cursor.
    cursor,
    /// The title bar's background.
    header_bar,
}

/// A built-in theme.
#[derive(Debug, PartialEq, Eq)]
pub struct Builtin {
    pub name: &'static str,
    pub palette: Palette,
}

/// How many colours the terminal can show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capability {
    /// 24-bit colour: `COLORTERM` is `truecolor` or `24bit`.
    Truecolor,
    /// The 256-colour palette: RGB is brought to the nearest index.
    Ansi256,
    /// `NO_COLOR` is set: bold, dim and reverse only.
    Monochrome,
}

impl Capability {
    /// From `NO_COLOR` and `COLORTERM` (https://no-color.org: set and not
    /// empty).
    pub fn detect(no_color: Option<&str>, colorterm: Option<&str>) -> Self {
        if no_color.is_some_and(|value| !value.is_empty()) {
            Self::Monochrome
        } else if matches!(colorterm, Some("truecolor" | "24bit")) {
            Self::Truecolor
        } else {
            Self::Ansi256
        }
    }

    fn color(self, color: Color) -> Color {
        match (self, color) {
            (Self::Ansi256, Color::Rgb(r, g, b)) => {
                Color::Indexed(ansi_colours::ansi256_from_rgb((r, g, b)))
            }
            _ => color,
        }
    }
}

/// Which theme the TUI draws with and what can change it: the name, the
/// `[tui.colors]` overrides, the terminal's capability and where the
/// palette's choice is saved.
#[derive(Clone, Debug)]
pub struct ThemeChoice {
    pub builtin: &'static Builtin,
    pub overrides: Vec<(&'static str, Color)>,
    pub capability: Capability,
    pub config_file: Option<std::path::PathBuf>,
}

impl Default for ThemeChoice {
    fn default() -> Self {
        Self {
            builtin: &BUILTIN[0],
            overrides: Vec::new(),
            capability: Capability::Ansi256,
            config_file: None,
        }
    }
}

impl ThemeChoice {
    /// The styles for the chosen theme, with the overrides on top.
    pub fn theme(&self) -> Theme {
        if self.capability == Capability::Monochrome {
            return Theme::monochrome();
        }
        let mut palette = self.builtin.palette.clone();
        for (role, color) in &self.overrides {
            if let Some(slot) = palette.role_mut(role) {
                *slot = *color;
            }
        }
        Theme::new(&palette, self.capability)
    }
}

/// The built-in theme called `name`.
pub fn builtin(name: &str) -> Option<&'static Builtin> {
    BUILTIN.iter().find(|theme| theme.name == name)
}

/// The built-in themes' names, default first.
pub fn names() -> Vec<&'static str> {
    BUILTIN.iter().map(|theme| theme.name).collect()
}

/// The styles `ui` draws with, one per role plus a few made from them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Theme {
    /// The whole screen and every modal: the background and text colour.
    pub base: Style,
    pub text: Style,
    /// Text in bold: a task's title in the detail pane, folder headings.
    pub strong: Style,
    pub text_dim: Style,
    pub text_muted: Style,
    pub border: Style,
    pub border_focused: Style,
    pub title: Style,
    /// An unfocused pane's title.
    pub title_unfocused: Style,
    /// The selected row in the focused pane.
    pub selection: Style,
    /// The selected row in another pane.
    pub selection_unfocused: Style,
    pub accent: Style,
    /// A key in the hints, help and palette; the mark on tasks `v`
    /// selected.
    pub key: Style,
    pub overdue: Style,
    pub due_today: Style,
    pub important: Style,
    pub completed: Style,
    pub sync_pending: Style,
    pub sync_unknown: Style,
    pub sync_failed: Style,
    pub error: Style,
    pub warning: Style,
    pub banner_error: Style,
    pub banner_info: Style,
    pub search_match: Style,
    pub link: Style,
    /// The character under the line editor's cursor.
    pub cursor: Style,
    /// The cursor's glyph past the end of the text.
    pub cursor_glyph: Style,
    pub header_bar: Style,
}

impl Default for Theme {
    fn default() -> Self {
        ThemeChoice::default().theme()
    }
}

impl Theme {
    pub fn new(palette: &Palette, capability: Capability) -> Self {
        let color = |color: Color| capability.color(color);
        let fg = |value: Color| match value {
            Color::Reset => Style::default(),
            value => Style::default().fg(color(value)),
        };
        // Secondary roles: the terminal's own text colour, dimmed, when the
        // palette leaves them to it. The colour is set, not inherited, so
        // an unfocused pane's title isn't the grey of its border dimmed.
        let soft = |value: Color| match value {
            Color::Reset => Style::default()
                .fg(Color::Reset)
                .add_modifier(Modifier::DIM),
            value => fg(value),
        };
        let bg = |value: Color| match value {
            Color::Reset => Style::default(),
            value => Style::default().bg(color(value)),
        };
        let selection = match palette.selection_bg {
            // No background to mark it with: the terminal's own inverse.
            Color::Reset => Style::default().add_modifier(Modifier::REVERSED),
            value => bg(value).patch(fg(palette.selection_fg)),
        };
        let cursor = fg(palette.cursor);
        Self {
            base: bg(palette.background).patch(fg(palette.text)),
            text: fg(palette.text),
            strong: fg(palette.text).add_modifier(Modifier::BOLD),
            text_dim: soft(palette.text_dim),
            text_muted: soft(palette.text_muted),
            border: fg(palette.border),
            border_focused: fg(palette.border_focused),
            title: fg(palette.title).add_modifier(Modifier::BOLD),
            title_unfocused: soft(palette.text_dim).add_modifier(Modifier::BOLD),
            selection,
            selection_unfocused: Style::default().add_modifier(Modifier::BOLD),
            accent: fg(palette.accent),
            key: fg(palette.accent).add_modifier(Modifier::BOLD),
            overdue: fg(palette.overdue),
            due_today: fg(palette.due_today),
            important: fg(palette.important),
            completed: soft(palette.completed).add_modifier(Modifier::CROSSED_OUT),
            sync_pending: soft(palette.sync_pending),
            sync_unknown: fg(palette.sync_unknown),
            sync_failed: fg(palette.sync_failed),
            error: fg(palette.error),
            warning: fg(palette.warning),
            banner_error: fg(palette.banner_error).add_modifier(Modifier::BOLD),
            banner_info: fg(palette.banner_info),
            search_match: fg(palette.search_match).add_modifier(Modifier::BOLD),
            link: fg(palette.link).add_modifier(Modifier::UNDERLINED),
            cursor: cursor.add_modifier(Modifier::REVERSED),
            cursor_glyph: cursor,
            header_bar: bg(palette.header_bar),
        }
    }

    /// `NO_COLOR`: no colour at all; bold, dim and reverse carry the
    /// meaning instead.
    pub fn monochrome() -> Self {
        let plain = Style::default();
        let bold = plain.add_modifier(Modifier::BOLD);
        let dim = plain.add_modifier(Modifier::DIM);
        let reversed = plain.add_modifier(Modifier::REVERSED);
        Self {
            base: plain,
            text: plain,
            strong: bold,
            text_dim: dim,
            text_muted: dim,
            border: dim,
            border_focused: plain,
            title: bold,
            title_unfocused: bold,
            selection: reversed,
            selection_unfocused: bold,
            accent: plain,
            key: bold,
            overdue: bold,
            due_today: plain,
            important: bold,
            completed: dim.add_modifier(Modifier::CROSSED_OUT),
            sync_pending: dim,
            sync_unknown: bold,
            sync_failed: bold,
            error: bold,
            warning: bold,
            banner_error: bold,
            banner_info: plain,
            search_match: bold,
            link: plain.add_modifier(Modifier::UNDERLINED),
            cursor: reversed,
            cursor_glyph: plain,
            header_bar: plain,
        }
    }
}

#[cfg(test)]
mod tests;
