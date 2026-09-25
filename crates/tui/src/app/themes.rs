//! The theme picker, from the palette's "Theme…": each theme is drawn as
//! the cursor reaches it, Enter keeps it (the runner saves it to
//! config.toml) and Esc puts the one before back.

use super::{App, Level, Mode};
use crate::theme::{BUILTIN, Builtin, ThemeChoice, ThemeError};

impl App {
    /// Draw with `choice` from now on.
    pub fn with_theme(mut self, choice: ThemeChoice) -> Self {
        self.theme = choice.theme();
        self.theme_choice = choice;
        self
    }

    pub(super) fn open_themes(&mut self) {
        let before = self.theme_choice.builtin;
        let index = BUILTIN
            .iter()
            .position(|theme| std::ptr::eq(theme, before))
            .unwrap_or(0);
        self.mode = Mode::Themes { index, before };
    }

    /// Up and Down: move to the next theme and draw with it.
    pub(super) fn step_theme(&mut self, down: bool) {
        let Mode::Themes { index, .. } = &mut self.mode else {
            return;
        };
        *index = if down {
            (*index + 1).min(BUILTIN.len().saturating_sub(1))
        } else {
            index.saturating_sub(1)
        };
        let theme = &BUILTIN[*index];
        self.preview(theme);
    }

    /// Enter: keep the theme drawn now, and have it saved.
    pub(super) fn keep_theme(&mut self) {
        self.mode = Mode::Normal;
        self.local = Some(super::LocalEffect::SaveTheme);
    }

    /// Esc: back to the theme from before the picker opened.
    pub(super) fn revert_theme(&mut self) {
        if let Mode::Themes { before, .. } = std::mem::replace(&mut self.mode, Mode::Normal) {
            self.preview(before);
        }
    }

    fn preview(&mut self, builtin: &'static Builtin) {
        self.theme_choice.builtin = builtin;
        self.theme = self.theme_choice.theme();
    }

    /// How saving the kept theme went, from the runner.
    pub fn theme_saved(&mut self, result: Result<(), ThemeError>) {
        let name = self.theme_choice.builtin.name;
        match result {
            Ok(()) => self.show(Level::Info, &format!("Theme: {name}, saved to config.toml")),
            Err(error) => self.show(
                Level::Error,
                &format!("Theme {name} is on for now, but {error}"),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::Action;
    use crate::app::tests::{act, seeded};
    use crate::app::{Msg, palette::Command};
    use crate::keybindings::Context;

    fn open_from_the_palette(app: &mut App) {
        act(app, Action::Palette);
        for ch in "theme".chars() {
            app.update(Msg::Char(ch));
        }
        let first = app.palette_items("theme").remove(0);
        assert_eq!(first.command, Command::Themes);
        act(app, Action::Submit);
    }

    #[test]
    fn the_palette_opens_the_picker_on_the_current_theme() {
        let mut app = seeded();
        open_from_the_palette(&mut app);
        assert_eq!(
            app.mode,
            Mode::Themes {
                index: 0,
                before: &BUILTIN[0]
            }
        );
        assert_eq!(app.context(), Context::Themes);
    }

    #[test]
    fn moving_previews_and_escape_reverts() {
        let mut app = seeded();
        let before = app.theme.clone();
        open_from_the_palette(&mut app);
        act(&mut app, Action::MoveDown);
        assert_eq!(app.theme_choice.builtin.name, "catppuccin-mocha");
        assert_ne!(app.theme, before, "drawn with the theme under the cursor");
        for _ in 0..BUILTIN.len() + 2 {
            act(&mut app, Action::MoveDown);
        }
        assert_eq!(
            app.theme_choice.builtin.name,
            BUILTIN[BUILTIN.len() - 1].name
        );
        act(&mut app, Action::Cancel);
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.theme_choice.builtin.name, "terminal");
        assert_eq!(app.theme, before);
        assert_eq!(app.local, None, "nothing saved");
    }

    #[test]
    fn enter_keeps_the_theme_and_asks_for_it_to_be_saved() {
        let mut app = seeded();
        open_from_the_palette(&mut app);
        act(&mut app, Action::MoveDown);
        act(&mut app, Action::MoveDown);
        let effects = act(&mut app, Action::Submit);
        assert!(effects.is_empty(), "nothing for the daemon");
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.theme_choice.builtin.name, "catppuccin-latte");
        assert_eq!(app.local, Some(crate::app::LocalEffect::SaveTheme));
    }

    #[test]
    fn a_failed_save_keeps_the_theme_and_says_why() {
        let mut app = seeded();
        app.theme_choice.builtin = crate::theme::builtin("nord").expect("nord");
        app.theme_saved(Err(ThemeError::Save {
            path: "/nowhere/config.toml".into(),
            message: "permission denied".into(),
        }));
        let banner = app.banner.as_ref().expect("a banner");
        assert_eq!(banner.level, Level::Error);
        assert!(banner.text.contains("nord") && banner.text.contains("permission denied"));
        app.theme_saved(Ok(()));
        assert_eq!(app.banner.as_ref().map(|b| b.level), Some(Level::Info));
    }
}
