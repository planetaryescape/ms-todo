// Adapted from mxr crates/tui/src/keybindings.rs @ dfb23d10138b1cfc24f8ea7450d3426e5e4da37a
// (unchanged at fec4aefc): `KeyPress`, `parse_key_string`, and hints and
// help read from the same default table. Changes: one static table of (context, key, action, label) rather
// than per-view maps of action names, since ms-todo has no keys.toml yet;
// single keys only (`g` is top, not `gg`, and the field picker is a
// mode, not an `e t` chord); named keys for arrows and Backspace.

//! The one keybinding registry: every key the TUI answers is in [`BINDINGS`],
//! and the hint bar and the help screen are read from it, so they can't
//! drift from what the keys do.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::action::Action;
use crate::app::edit::Field;

/// Where a key is pressed. Each has its own bindings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Context {
    /// The sidebar.
    Sidebar,
    /// The task list.
    Tasks,
    /// The detail pane, where j and k move between the fields.
    Detail,
    /// Typing a filter or a field's new value on one line. Other keys go
    /// to the line editor ([`EDITOR_KEYS`]).
    Prompt,
    /// Quick add's modal: a new task, read as it's typed. Other keys go to
    /// the line editor.
    Adding,
    /// Typing notes, where Enter is a new line.
    Notes,
    /// Typing the folder to move a list into, with suggestions.
    Folder,
    /// Picking which field of a task to edit.
    Fields,
    /// Picking an importance level.
    Importance,
    /// The command palette. Other printable keys are its query.
    Palette,
    /// The diagnostics page.
    Diagnostics,
    /// The inline delete confirmation.
    Confirm,
    /// The recurring-completion candidate picker.
    Picker,
    /// The theme picker, previewing each theme.
    Themes,
    /// A task's links, to open or copy one.
    Links,
    /// Picking the list to move tasks to. Other printable keys are its
    /// query.
    MoveTo,
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

const BROWSE: &[Context] = &[Context::Sidebar, Context::Tasks, Context::Detail];
const TASKS: &[Context] = &[Context::Tasks, Context::Detail];
const SIDEBAR: &[Context] = &[Context::Sidebar];
const LISTS: &[Context] = &[
    Context::Sidebar,
    Context::Tasks,
    Context::Detail,
    Context::Picker,
    Context::Themes,
    Context::Links,
    Context::Diagnostics,
];
const PALETTE: &[Context] = &[Context::Palette];
const PROMPTS: &[Context] = &[
    Context::Prompt,
    Context::Adding,
    Context::Notes,
    Context::Folder,
];
const ADDING: &[Context] = &[Context::Adding];
const NOTES: &[Context] = &[Context::Notes];
const FIELDS: &[Context] = &[Context::Fields];
const FOLDER: &[Context] = &[Context::Folder];
const LEVELS: &[Context] = &[Context::Importance];
const DIAGNOSTICS: &[Context] = &[Context::Diagnostics];
const MOVE_TO: &[Context] = &[Context::MoveTo];

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
    bind(SIDEBAR, "Enter", Action::Open, "Open or fold", true),
    bind(SIDEBAR, "Space", Action::Open, "Open or fold", false),
    bind(BROWSE, "Tab", Action::FocusNext, "Next pane", false),
    bind(BROWSE, "a", Action::Add, "Add", true),
    bind(TASKS, "x", Action::ToggleComplete, "Done", true),
    bind(TASKS, "e", Action::Edit, "Edit", true),
    bind(
        &[Context::Detail],
        "Enter",
        Action::EditHere,
        "Edit this field",
        false,
    ),
    bind(TASKS, "d", Action::Delete, "Delete", true),
    bind(TASKS, "S", Action::SetDue, "Set due date\u{2026}", false),
    bind(
        TASKS,
        "R",
        Action::RescheduleOverdue,
        "Reschedule overdue to\u{2026}",
        false,
    ),
    bind(TASKS, "m", Action::MoveTasks, "Move to list\u{2026}", false),
    bind(TASKS, "v", Action::ToggleSelect, "Select", true),
    bind(TASKS, "V", Action::SelectAll, "Select all", false),
    bind(BROWSE, "u", Action::Undo, "Undo", true),
    bind(BROWSE, "/", Action::Filter, "Filter", true),
    bind(
        TASKS,
        "Esc",
        Action::Clear,
        "Clear selection or filter",
        false,
    ),
    bind(BROWSE, ":", Action::Palette, "Palette", true),
    bind(BROWSE, "r", Action::Sync, "Sync", true),
    bind(BROWSE, "D", Action::Diagnostics, "Diagnostics", false),
    bind(
        BROWSE,
        "M",
        Action::MoveToFolder,
        "Move list to folder\u{2026}",
        false,
    ),
    bind(BROWSE, "?", Action::Help, "Help", true),
    bind(BROWSE, "q", Action::Quit, "Quit", true),
    bind(BROWSE, "Ctrl-c", Action::Quit, "Quit", false),
    // After Quit, so a narrow hint bar loses these first.
    bind(TASKS, "o", Action::OpenLink, "Open link", true),
    bind(TASKS, "y", Action::CopyLink, "Copy link", true),
    bind(&[Context::Prompt], "Enter", Action::Submit, "Done", true),
    bind(ADDING, "Enter", Action::Submit, "Add", true),
    bind(ADDING, "Tab", Action::Complete, "Complete", true),
    bind(ADDING, "Ctrl-r", Action::ToggleParse, "Literal", true),
    bind(NOTES, "Ctrl-s", Action::Submit, "Save", true),
    bind(NOTES, "Alt-Enter", Action::Submit, "Save", false),
    bind(NOTES, "Enter", Action::Newline, "New line", true),
    bind(FOLDER, "Enter", Action::Submit, "Move", true),
    bind(FOLDER, "Tab", Action::Complete, "Complete", true),
    bind(PROMPTS, "Esc", Action::Cancel, "Cancel", true),
    bind(PROMPTS, "Backspace", Action::Backspace, "Erase", false),
    bind(PROMPTS, "Ctrl-c", Action::Cancel, "Cancel", false),
    bind(
        FIELDS,
        "t",
        Action::EditField(Field::Title),
        "Edit title",
        false,
    ),
    bind(
        FIELDS,
        "d",
        Action::EditField(Field::Due),
        "Edit due date",
        false,
    ),
    bind(
        FIELDS,
        "r",
        Action::EditField(Field::Reminder),
        "Edit reminder",
        false,
    ),
    bind(
        FIELDS,
        "i",
        Action::EditField(Field::Importance),
        "Set importance",
        false,
    ),
    bind(
        FIELDS,
        "n",
        Action::EditField(Field::Notes),
        "Edit notes",
        false,
    ),
    bind(
        FIELDS,
        "I",
        Action::CycleImportance,
        "Cycle importance",
        true,
    ),
    bind(FIELDS, "Esc", Action::Cancel, "Cancel", true),
    bind(FIELDS, "Ctrl-c", Action::Cancel, "Cancel", false),
    bind(LEVELS, "1", Action::SetImportance("1"), "High", true),
    bind(LEVELS, "2", Action::SetImportance("2"), "Normal", true),
    bind(LEVELS, "3", Action::SetImportance("3"), "Normal", true),
    bind(LEVELS, "4", Action::SetImportance("4"), "Low", true),
    bind(LEVELS, "h", Action::SetImportance("high"), "High", true),
    bind(LEVELS, "n", Action::SetImportance("normal"), "Normal", true),
    bind(LEVELS, "l", Action::SetImportance("low"), "Low", true),
    bind(LEVELS, "Esc", Action::Cancel, "Cancel", true),
    bind(LEVELS, "Ctrl-c", Action::Cancel, "Cancel", false),
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
    bind(&[Context::Links], "Enter", Action::Submit, "Open", true),
    bind(&[Context::Links], "o", Action::OpenLink, "Open", false),
    bind(&[Context::Links], "y", Action::CopyLink, "Copy", true),
    bind(&[Context::Links], "Esc", Action::Cancel, "Cancel", true),
    bind(&[Context::Links], "Ctrl-c", Action::Cancel, "Cancel", false),
    bind(&[Context::Themes], "Enter", Action::Submit, "Keep", true),
    bind(&[Context::Themes], "Esc", Action::Cancel, "Revert", true),
    bind(
        &[Context::Themes],
        "Ctrl-c",
        Action::Cancel,
        "Revert",
        false,
    ),
    bind(PALETTE, "Enter", Action::Submit, "Run", true),
    bind(PALETTE, "Esc", Action::Cancel, "Close", true),
    bind(PALETTE, "Backspace", Action::Backspace, "Erase", false),
    bind(PALETTE, "Down", Action::MoveDown, "Down", false),
    bind(PALETTE, "Up", Action::MoveUp, "Up", false),
    bind(PALETTE, "Ctrl-n", Action::MoveDown, "Down", false),
    bind(PALETTE, "Ctrl-p", Action::MoveUp, "Up", false),
    bind(PALETTE, "Ctrl-c", Action::Cancel, "Close", false),
    bind(MOVE_TO, "Enter", Action::Submit, "Move here", true),
    bind(MOVE_TO, "Esc", Action::Cancel, "Cancel", true),
    bind(MOVE_TO, "Backspace", Action::Backspace, "Erase", false),
    bind(MOVE_TO, "Down", Action::MoveDown, "Down", true),
    bind(MOVE_TO, "Up", Action::MoveUp, "Up", false),
    bind(MOVE_TO, "Ctrl-n", Action::MoveDown, "Down", false),
    bind(MOVE_TO, "Ctrl-p", Action::MoveUp, "Up", false),
    bind(MOVE_TO, "Ctrl-c", Action::Cancel, "Cancel", false),
    bind(DIAGNOSTICS, "r", Action::Refresh, "Refresh", true),
    bind(DIAGNOSTICS, "Esc", Action::Cancel, "Back", true),
    bind(DIAGNOSTICS, "q", Action::Cancel, "Back", false),
    bind(DIAGNOSTICS, "D", Action::Cancel, "Back", false),
    bind(&[Context::Help], "Esc", Action::Cancel, "Close", true),
    bind(&[Context::Help], "?", Action::Cancel, "Close", false),
    bind(&[Context::Help], "q", Action::Cancel, "Close", false),
];

/// What the line editor in every prompt does with the keys the table
/// doesn't bind, for help (`app/line_editor.rs` has the mapping).
pub const EDITOR_KEYS: &[(&str, &str)] = &[
    ("Left/Right", "Move (Home/End, Ctrl-a/e: line start/end)"),
    ("Alt-b/Alt-f", "Word back/forward (also Ctrl-Left/Right)"),
    ("Ctrl-w/u/k", "Delete word back, to line start, to end"),
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

/// Parse a key string like "j", "G", "Ctrl-c", "Alt-Enter" or "Down".
pub fn parse_key_string(key: &str) -> Result<KeyPress, String> {
    if let Some(rest) = key.strip_prefix("Alt-") {
        let mut press = parse_key_string(rest)?;
        press.modifiers |= KeyModifiers::ALT;
        return Ok(press);
    }
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
        "Space" => named(KeyCode::Char(' ')),
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
    let mut modifiers =
        key.modifiers & (KeyModifiers::CONTROL | KeyModifiers::SHIFT | KeyModifiers::ALT);
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

/// The first key bound to `action` in `context`.
pub fn key_for(context: Context, action: Action) -> Option<&'static str> {
    BINDINGS
        .iter()
        .find(|binding| binding.contexts.contains(&context) && binding.action == action)
        .map(|binding| binding.key)
}

/// The hint bar's `(keys, label)` pairs for `context`.
pub fn hints(context: Context) -> Vec<(String, &'static str)> {
    labelled(grouped(|binding| {
        binding.hint && binding.contexts.contains(&context)
    }))
}

/// Every browsing binding, for the help screen.
pub fn help_rows() -> Vec<(String, &'static str)> {
    labelled(grouped(|binding| {
        binding.contexts.iter().any(|c| BROWSE.contains(c))
    }))
}

/// What the command palette offers: every browsing action but moving
/// around and the palette itself, then each field the picker edits, with
/// its keys after the picker's, as `e t`; as `(keys, label, action)`.
pub fn commands() -> Vec<(String, &'static str, Action)> {
    let browsing = grouped(|binding| {
        binding.contexts.iter().any(|c| BROWSE.contains(c))
            && !matches!(
                binding.action,
                Action::MoveDown
                    | Action::MoveUp
                    | Action::JumpTop
                    | Action::JumpBottom
                    | Action::FocusLeft
                    | Action::FocusRight
                    | Action::FocusNext
                    // Only the sidebar's row, which the palette covers.
                    | Action::Open
                    | Action::Palette
                    // The palette names each field instead.
                    | Action::EditHere
            )
    });
    let fields = grouped(|binding| binding.contexts == FIELDS && binding.action != Action::Cancel)
        .into_iter()
        .map(|(keys, binding)| {
            let picker = key_for(Context::Tasks, Action::Edit).unwrap_or_default();
            (format!("{picker} {keys}"), binding)
        });
    browsing
        .into_iter()
        .chain(fields)
        .map(|(keys, binding)| (keys, binding.label, binding.action))
        .collect()
}

/// The bindings `wanted` picks, one row per label with its keys joined
/// as `j/Down` and the first binding of that label, in table order.
fn grouped(wanted: impl Fn(&Binding) -> bool) -> Vec<(String, &'static Binding)> {
    let mut rows: Vec<(String, &'static Binding)> = Vec::new();
    for binding in BINDINGS.iter().filter(|binding| wanted(binding)) {
        match rows
            .iter_mut()
            .find(|(_, first)| first.label == binding.label)
        {
            Some((keys, _)) => {
                keys.push('/');
                keys.push_str(binding.key);
            }
            None => rows.push((binding.key.to_owned(), binding)),
        }
    }
    rows
}

fn labelled(rows: Vec<(String, &'static Binding)>) -> Vec<(String, &'static str)> {
    rows.into_iter()
        .map(|(keys, binding)| (keys, binding.label))
        .collect()
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
            Some(Action::Open)
        );
        assert_eq!(
            resolve(
                Context::Sidebar,
                &key(KeyCode::Char(' '), KeyModifiers::NONE)
            ),
            Some(Action::Open)
        );
        assert_eq!(
            resolve(Context::Folder, &key(KeyCode::Tab, KeyModifiers::NONE)),
            Some(Action::Complete)
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
        assert!(help.contains(&("e".into(), "Edit")));
        assert!(help.contains(&("Enter".into(), "Edit this field")));
    }

    #[test]
    fn the_palette_offers_actions_not_movement() {
        let commands = commands();
        assert!(commands.contains(&("x".into(), "Done", Action::ToggleComplete)));
        assert!(commands.contains(&("D".into(), "Diagnostics", Action::Diagnostics)));
        // Each field the picker edits, by its keys.
        assert!(commands.contains(&("e d".into(), "Edit due date", Action::EditField(Field::Due))));
        assert!(commands.contains(&("e I".into(), "Cycle importance", Action::CycleImportance)));
        assert!(!commands.iter().any(|(keys, _, _)| keys == "e Esc"));
        assert!(
            !commands
                .iter()
                .any(|(_, _, action)| matches!(action, Action::MoveDown | Action::Palette))
        );
    }
}
