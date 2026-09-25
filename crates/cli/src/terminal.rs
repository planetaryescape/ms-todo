//! The global `--quiet` and `--no-color` (docs/blueprint/07-cli.md#global-flags):
//! set once, before a command runs, and read wherever a note, progress or
//! bold would be printed.

use std::sync::OnceLock;

use crate::output::OutputFormat;

static QUIET: OnceLock<bool> = OnceLock::new();
static NO_COLOR: OnceLock<bool> = OnceLock::new();

/// Record the flags. Called once, first thing.
pub fn configure(quiet: bool, no_color: bool) {
    let _ = QUIET.set(quiet);
    let _ = NO_COLOR.set(no_color);
}

/// `--quiet`: nothing on stderr but errors.
pub fn quiet() -> bool {
    QUIET.get().copied().unwrap_or(false)
}

/// Whether colour and bold are allowed: not `--no-color`, and no
/// `NO_COLOR` (https://no-color.org).
pub fn color_allowed() -> bool {
    !NO_COLOR.get().copied().unwrap_or(false)
        && std::env::var_os("NO_COLOR").is_none_or(|value| value.is_empty())
}

/// A `note:` line on stderr, unless `--quiet`.
pub fn note(message: &str) {
    if !quiet() {
        eprintln!("note: {message}");
    }
}

/// A `note:` for people: not in JSON, whose stdout and stderr are the
/// output contract.
pub fn note_unless_json(format: OutputFormat, message: &str) {
    if !matches!(format, OutputFormat::Json | OutputFormat::Jsonl) {
        note(message);
    }
}
