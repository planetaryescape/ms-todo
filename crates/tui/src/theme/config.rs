//! The theme's settings in config.toml (D-035): `[tui] theme` and the
//! per-role `[tui.colors]`, and saving the palette's choice back there
//! without disturbing the rest of the file.
//!
//! ```toml
//! [tui]
//! theme = "catppuccin-mocha"
//!
//! [tui.colors]
//! overdue = "#ff5f5f"
//! border = "dark-gray"
//! ```

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use ratatui::style::Color;
use toml_edit::{DocumentMut, Item, TableLike};

use super::{BUILTIN, Builtin, Capability, ROLES, ThemeChoice, builtin, names};

#[derive(Debug, thiserror::Error)]
pub enum ThemeError {
    #[error("unknown theme \"{name}\" {from}; the themes are: {}", names().join(", "))]
    UnknownTheme { name: String, from: String },
    #[error("{}: {message}", path.display())]
    Config { path: PathBuf, message: String },
    #[error("cannot save the theme to {}: {message}", path.display())]
    Save { path: PathBuf, message: String },
}

/// The theme to draw with: `flag` (`--theme`) wins over `[tui] theme`,
/// and `NO_COLOR` and `COLORTERM` decide how much colour it gets.
pub fn load(config_file: &Path, flag: Option<&str>) -> Result<ThemeChoice, ThemeError> {
    let no_color = std::env::var("NO_COLOR").ok();
    let colorterm = std::env::var("COLORTERM").ok();
    let capability = Capability::detect(no_color.as_deref(), colorterm.as_deref());
    load_from(config_file, flag, capability)
}

/// [`load`] with the environment's answer passed in, for tests.
pub(crate) fn load_from(
    config_file: &Path,
    flag: Option<&str>,
    capability: Capability,
) -> Result<ThemeChoice, ThemeError> {
    let settings = read(config_file)?;
    let name = match (flag, &settings.theme) {
        (Some(name), _) => known(name, "(from --theme)")?,
        (None, Some(name)) => known(name, &format!("(tui.theme in {})", config_file.display()))?,
        (None, None) => &BUILTIN[0],
    };
    Ok(ThemeChoice {
        builtin: name,
        overrides: settings.colors,
        capability,
        config_file: Some(config_file.to_owned()),
    })
}

fn known(name: &str, from: &str) -> Result<&'static Builtin, ThemeError> {
    builtin(name).ok_or_else(|| ThemeError::UnknownTheme {
        name: name.to_owned(),
        from: from.to_owned(),
    })
}

#[derive(Debug, Default)]
struct Settings {
    theme: Option<String>,
    colors: Vec<(&'static str, Color)>,
}

/// `[tui]` from config.toml; nothing when there's no file. Every key is
/// checked, so a typo is an error naming it rather than a setting that
/// silently does nothing.
fn read(path: &Path) -> Result<Settings, ThemeError> {
    let invalid = |message: String| ThemeError::Config {
        path: path.to_owned(),
        message,
    };
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Settings::default()),
        Err(error) => return Err(invalid(error.to_string())),
    };
    let document: DocumentMut = raw.parse().map_err(|error| invalid(format!("{error}")))?;
    let Some(tui) = document.get("tui") else {
        return Ok(Settings::default());
    };
    let tui = tui
        .as_table_like()
        .ok_or_else(|| invalid("tui must be a table: [tui]".into()))?;
    let mut settings = Settings::default();
    for (key, item) in tui.iter() {
        match key {
            "theme" => {
                let name = item.as_str().ok_or_else(|| {
                    invalid("tui.theme must be a string, such as \"catppuccin-mocha\"".into())
                })?;
                settings.theme = Some(name.to_owned());
            }
            "colors" => settings.colors = colors(item, &invalid)?,
            other => {
                return Err(invalid(format!(
                    "tui.{other} is not a setting; [tui] takes theme and [tui.colors]"
                )));
            }
        }
    }
    Ok(settings)
}

fn colors(
    item: &Item,
    invalid: &impl Fn(String) -> ThemeError,
) -> Result<Vec<(&'static str, Color)>, ThemeError> {
    let table: &dyn TableLike = item
        .as_table_like()
        .ok_or_else(|| invalid("tui.colors must be a table: [tui.colors]".into()))?;
    table
        .iter()
        .map(|(key, item)| {
            let role = ROLES.iter().find(|role| **role == key).ok_or_else(|| {
                invalid(format!(
                    "tui.colors.{key} is not a role; the roles are: {}",
                    ROLES.join(", ")
                ))
            })?;
            let value = item.as_str().ok_or_else(|| {
                invalid(format!(
                    "tui.colors.{key} must be a string, such as \"#f38ba8\" or \"red\""
                ))
            })?;
            let color = Color::from_str(value).map_err(|_| {
                invalid(format!(
                    "tui.colors.{key} = \"{value}\" is not a colour: use #rrggbb, an ANSI \
                     name such as red, bright-blue or dark-gray, a 256-colour index, or \
                     reset for the terminal's own"
                ))
            })?;
            Ok((*role, color))
        })
        .collect()
}

/// Set `[tui] theme = "name"` in config.toml, keeping the rest of the
/// file, its comments and layout as they were, and creating it if need be.
pub fn save(path: &Path, name: &str) -> Result<(), ThemeError> {
    let failed = |message: String| ThemeError::Save {
        path: path.to_owned(),
        message,
    };
    // A config.toml linked from a dotfiles repo stays a link: the file it
    // points to is the one rewritten.
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let raw = match std::fs::read_to_string(&target) {
        Ok(raw) => raw,
        Err(error) if error.kind() == ErrorKind::NotFound => String::new(),
        Err(error) => return Err(failed(error.to_string())),
    };
    let mut document: DocumentMut = raw.parse().map_err(|error| failed(format!("{error}")))?;
    let tui = document
        .entry("tui")
        .or_insert_with(toml_edit::table)
        .as_table_like_mut()
        .ok_or_else(|| failed("tui must be a table: [tui]".into()))?;
    match tui.get_mut("theme").and_then(Item::as_value_mut) {
        // Replaced in place, keeping the comments around it.
        Some(value) => {
            let decor = value.decor().clone();
            *value = name.into();
            *value.decor_mut() = decor;
        }
        None => {
            tui.insert("theme", toml_edit::value(name));
        }
    }
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir).map_err(|error| failed(error.to_string()))?;
    }
    // Written beside it and renamed over it, so a crash can't leave half
    // a config behind.
    let staged = target.with_extension("toml.tmp");
    std::fs::write(&staged, document.to_string()).map_err(|error| failed(error.to_string()))?;
    std::fs::rename(&staged, &target).map_err(|error| failed(error.to_string()))
}
