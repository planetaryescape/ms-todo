// Adapted from mxr crates/tui/src/keybindings.rs @ dfb23d10138b1cfc24f8ea7450d3426e5e4da37a
// (unchanged at fec4aefc): `KeyPress`, `parse_key_string`, and hints and
// help read from the same default table. Changes: one static table of (context, key, action, label) rather
// than per-view maps of action names, since ms-todo has no keys.toml yet;
// single keys only (`g` is top, not `gg`); named keys for arrows and
// Backspace.

//! The one keybinding registry: every key the TUI answers is in [`BINDINGS`],
//! and the hint bar and the help screen are read from it, so they can't
//! drift from what the keys do.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::action::Action;

/// Where a key is pressed. Each has its own bindings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Context {
    /// The sidebar.
    Sidebar,
    /// The task list and the detail pane.
    Tasks,
    /// Typing a new task or a filter. Other printable keys are text.
    Prompt,
    /// The inline delete confirmation.
    Confirm,
    /// The recurring-completion candidate picker.
    Picker,
    Help,
}

/// A single key with its modifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeyPress {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

pub struct Binding {
    pub contexts: &'static [Context],
    pub key: &'static str,
    pub action: Action,
    pub label: &'static str,
    /// Shown in the hint bar in these contexts.
    pub hint: bool,
}

const BROWSE: &[Context] = &[Context::Sidebar, Context::Tasks];
const TASKS: &[Context] = &[Context::Tasks];
const SIDEBAR: &[Context] = &[Context::Sidebar];
const LISTS: &[Context] = &[Context::Sidebar, Context::Tasks, Context::Picker];

/// Every binding, in the order help shows them.
pub const BINDINGS: &[Binding] = &[
    bind(LISTS, "j", Action::MoveDown, "Down", false),
    bind(LISTS, "Down", Action::MoveDown, "Down", false),
    bind(LISTS, "k", Action::MoveUp, "Up", false),
    bind(LISTS, "Up", Action::MoveUp, "Up", false),
    bind(BROWSE, "g", Action::JumpTop, "Top", false),
    bind(BROWSE, "G", Action::JumpBottom, "Bottom", false),
    bind(BROWSE, "h", Action::FocusLeft, "Left pane", false),
    bind(BROWSE, "l", Action::FocusRight, "Right pane", false),
    bind(SIDEBAR, "Enter", Action::FocusRight, "Open", true),
    bind(BROWSE, "Tab", Action::FocusNext, "Next pane", false),
    bind(BROWSE, "a", Action::Add, "Add", true),
    bind(TASKS, "x", Action::ToggleComplete, "Done", true),
    bind(TASKS, "d", Action::Delete, "Delete", true),
    bind(BROWSE, "u", Action::Undo, "Undo", true),
    bind(BROWSE, "/", Action::Filter, "Filter", true),
    bind(TASKS, "Esc", Action::ClearFilter, "Clear filter", false),
    bind(BROWSE, "r", Action::Sync, "Sync", true),
    bind(BROWSE, "?", Action::Help, "Help", true),
    bind(BROWSE, "q", Action::Quit, "Quit", true),
    bind(BROWSE, "Ctrl-c", Action::Quit, "Quit", false),
    bind(&[Context::Prompt], "Enter", Action::Submit, "Done", true),
    bind(&[Context::Prompt], "Esc", Action::Cancel, "Cancel", true),
    bind(
        &[Context::Prompt],
        "Backspace",
        Action::Backspace,
        "Erase",
        false,
    ),
    bind(
        &[Context::Prompt],
        "Ctrl-c",
        Action::Cancel,
        "Cancel",
        false,
    ),
    bind(&[Context::Confirm], "y", Action::Confirm, "Delete", true),
    bind(&[Context::Confirm], "n", Action::Cancel, "Keep", true),
    bind(&[Context::Confirm], "Esc", Action::Cancel, "Keep", false),
    bind(
        &[Context::Picker],
        "Enter",
        Action::Submit,
        "Delete this copy",
        true,
    ),
    bind(&[Context::Picker], "Esc", Action::Cancel, "Cancel", true),
    bind(&[Context::Help], "Esc", Action::Cancel, "Close", true),
    bind(&[Context::Help], "?", Action::Cancel, "Close", false),
    bind(&[Context::Help], "q", Action::Cancel, "Close", false),
];

const fn bind(
    contexts: &'static [Context],
    key: &'static str,
    action: Action,
    label: &'static str,
    hint: bool,
) -> Binding {
    Binding {
        contexts,
        key,
        action,
        label,
        hint,
    }
}

/// Parse a key string like "j", "G", "Ctrl-c", "Enter" or "Down".
pub fn parse_key_string(key: &str) -> Result<KeyPress, String> {
    let named = |code| KeyPress {
        code,
        modifiers: KeyModifiers::NONE,
    };
    if let Some(rest) = key.strip_prefix("Ctrl-") {
        let ch = rest.chars().next().ok_or("missing a key after Ctrl-")?;
        return Ok(KeyPress {
            code: KeyCode::Char(ch),
            modifiers: KeyModifiers::CONTROL,
        });
    }
    Ok(match key {
        "Enter" => named(KeyCode::Enter),
        "Esc" | "Escape" => named(KeyCode::Esc),
        "Tab" => named(KeyCode::Tab),
        "Backspace" => named(KeyCode::Backspace),
        "Up" => named(KeyCode::Up),
        "Down" => named(KeyCode::Down),
        _ => {
            let mut chars = key.chars();
            let (Some(ch), None) = (chars.next(), chars.next()) else {
                return Err(format!("{key:?} isn't one key"));
            };
            KeyPress {
                code: KeyCode::Char(ch),
                modifiers: if ch.is_uppercase() {
                    KeyModifiers::SHIFT
                } else {
                    KeyModifiers::NONE
                },
            }
        }
    })
}

/// What `key` does in `context`, if anything. Key releases and repeats
/// of other kinds are ignored; a shifted letter matches its binding
/// whether or not the terminal reports Shift.
pub fn resolve(context: Context, key: &KeyEvent) -> Option<Action> {
    if key.kind == KeyEventKind::Release {
        return None;
    }
    let pressed = normalize(key);
    BINDINGS
        .iter()
        .filter(|binding| binding.contexts.contains(&context))
        .find(|binding| parse_key_string(binding.key).is_ok_and(|bound| bound == pressed))
        .map(|binding| binding.action)
}

fn normalize(key: &KeyEvent) -> KeyPress {
    let mut modifiers = key.modifiers & (KeyModifiers::CONTROL | KeyModifiers::SHIFT);
    if let KeyCode::Char(ch) = key.code
        && !modifiers.contains(KeyModifiers::CONTROL)
    {
        // Terminals disagree on whether "G" arrives with Shift.
        modifiers = if ch.is_uppercase() {
            KeyModifiers::SHIFT
        } else {
            KeyModifiers::NONE
        };
    }
    KeyPress {
        code: key.code,
        modifiers,
    }
}

/// The hint bar's `(keys, label)` pairs for `context`.
pub fn hints(context: Context) -> Vec<(String, &'static str)> {
    grouped(|binding| binding.hint && binding.contexts.contains(&context))
}

/// Every browsing binding, for the help screen.
pub fn help_rows() -> Vec<(String, &'static str)> {
    grouped(|binding| binding.contexts.iter().any(|c| BROWSE.contains(c)))
}

/// The bindings `wanted` picks as `(keys, label)`, one row per label with
/// its keys joined as `j/Down`, in table order.
fn grouped(wanted: impl Fn(&Binding) -> bool) -> Vec<(String, &'static str)> {
    let mut rows: Vec<(String, &'static str)> = Vec::new();
    for binding in BINDINGS.iter().filter(|binding| wanted(binding)) {
        match rows.iter_mut().find(|(_, label)| *label == binding.label) {
            Some((keys, _)) => {
                keys.push('/');
                keys.push_str(binding.key);
            }
            None => rows.push((binding.key.to_owned(), binding.label)),
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn every_binding_parses() {
        for binding in BINDINGS {
            assert!(parse_key_string(binding.key).is_ok(), "{}", binding.key);
        }
    }

    #[test]
    fn no_key_does_two_things_in_one_context() {
        for (index, binding) in BINDINGS.iter().enumerate() {
            for other in &BINDINGS[index + 1..] {
                let shared = binding.contexts.iter().any(|c| other.contexts.contains(c));
                assert!(
                    !(shared && binding.key == other.key && binding.action != other.action),
                    "{} is bound twice",
                    binding.key
                );
            }
        }
    }

    #[test]
    fn keys_resolve_by_context() {
        let x = key(KeyCode::Char('x'), KeyModifiers::NONE);
        assert_eq!(resolve(Context::Tasks, &x), Some(Action::ToggleComplete));
        assert_eq!(resolve(Context::Sidebar, &x), None);
        assert_eq!(resolve(Context::Prompt, &x), None, "text in a prompt");
        // "G" with or without Shift reported.
        for modifiers in [KeyModifiers::NONE, KeyModifiers::SHIFT] {
            assert_eq!(
                resolve(Context::Tasks, &key(KeyCode::Char('G'), modifiers)),
                Some(Action::JumpBottom)
            );
        }
        assert_eq!(
            resolve(
                Context::Tasks,
                &key(KeyCode::Char('c'), KeyModifiers::CONTROL)
            ),
            Some(Action::Quit)
        );
        assert_eq!(
            resolve(Context::Sidebar, &key(KeyCode::Enter, KeyModifiers::NONE)),
            Some(Action::FocusRight)
        );
    }

    #[test]
    fn hints_and_help_come_from_the_table() {
        let hints = hints(Context::Tasks);
        assert!(hints.contains(&("x".into(), "Done")));
        assert!(!hints.iter().any(|(_, label)| *label == "Open"));
        let help = help_rows();
        assert!(help.contains(&("j/Down".into(), "Down")));
        assert!(help.contains(&("q/Ctrl-c".into(), "Quit")));
    }
}
