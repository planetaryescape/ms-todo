//! The help screen's content and layout, read from the keybinding
//! registry: every binding, grouped by where it's pressed, in two columns
//! when the terminal is wide enough and one otherwise, wrapped rather
//! than cut. `App::update` scrolls with the same layout the frame is
//! drawn from, so scrolling stops at the last row whatever the size.

use ratatui::layout::Size;
use unicode_width::UnicodeWidthStr;

use crate::action::Action;
use crate::keybindings::{BINDINGS, Binding, Context, EDITOR_KEYS};

/// Help's parts, in the order shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Navigation,
    Tasks,
    Detail,
    Prompts,
    Views,
}

impl Section {
    const ALL: [Self; 5] = [
        Self::Navigation,
        Self::Tasks,
        Self::Detail,
        Self::Prompts,
        Self::Views,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::Navigation => "Navigation",
            Self::Tasks => "Tasks",
            Self::Detail => "Detail pane",
            Self::Prompts => "Prompts and editing",
            Self::Views => "Views and palette",
        }
    }
}

/// Rows under one heading of a section; browsing keys have none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub heading: Option<&'static str>,
    /// `(keys, label)`: one row per label, its keys joined as `j/Down`.
    pub rows: Vec<(String, &'static str)>,
}

/// Where each non-browsing context's keys are listed, under what heading.
/// Exhaustive, so a new context can't reach help without a place.
fn place_context(context: Context) -> (Section, &'static str) {
    match context {
        Context::Sidebar => (Section::Navigation, "Sidebar"),
        Context::Tasks => (Section::Tasks, "Task list"),
        Context::Detail | Context::Steps => (Section::Detail, "Detail pane"),
        Context::Prompt => (Section::Prompts, "Filter, due date or a field"),
        Context::Adding => (Section::Prompts, "Quick add"),
        Context::Notes => (Section::Prompts, "Notes"),
        Context::Folder => (Section::Prompts, "Move list to folder"),
        Context::Fields => (Section::Prompts, "Picking a field (e)"),
        Context::Importance => (Section::Prompts, "Importance"),
        Context::Confirm => (Section::Prompts, "Confirming a delete"),
        Context::Palette => (Section::Views, "Palette (:)"),
        Context::MoveTo => (Section::Views, "Move to list (m)"),
        Context::Picker => (Section::Views, "Undoing a recurring task"),
        Context::Themes => (Section::Views, "Theme picker"),
        Context::Links => (Section::Views, "Links"),
        Context::Diagnostics => (Section::Views, "Diagnostics (D)"),
        Context::Help => (Section::Views, "This help"),
    }
}

fn is_browsing(context: &Context) -> bool {
    matches!(
        context,
        Context::Sidebar | Context::Tasks | Context::Detail | Context::Steps
    )
}

fn is_prompt(context: &Context) -> bool {
    matches!(
        context,
        Context::Prompt | Context::Adding | Context::Notes | Context::Folder
    )
}

/// The section and heading a binding is listed under.
fn place(binding: &Binding) -> (Section, Option<&'static str>) {
    let contexts = binding.contexts;
    if contexts.iter().any(is_browsing) {
        let section = if contexts
            .iter()
            .all(|c| matches!(c, Context::Detail | Context::Steps))
        {
            Section::Detail
        } else {
            match binding.action {
                Action::MoveDown
                | Action::MoveUp
                | Action::JumpTop
                | Action::JumpBottom
                | Action::FocusLeft
                | Action::FocusRight
                | Action::FocusNext
                | Action::Open
                | Action::Help
                | Action::Quit => Section::Navigation,
                Action::Palette | Action::Diagnostics => Section::Views,
                _ => Section::Tasks,
            }
        };
        return (section, None);
    }
    let Some(first) = contexts.first() else {
        return (Section::Views, None);
    };
    let (section, heading) = place_context(*first);
    if contexts.len() > 1 && contexts.iter().all(is_prompt) {
        return (Section::Prompts, Some("Any prompt"));
    }
    (section, Some(heading))
}

/// Every binding in the registry, then the line editor's keys, by section.
pub fn content() -> Vec<(Section, Vec<Group>)> {
    let mut sections: Vec<(Section, Vec<Group>)> =
        Section::ALL.iter().map(|&s| (s, Vec::new())).collect();
    let mut add = |section: Section, heading: Option<&'static str>, key: &str, label| {
        let Some((_, groups)) = sections.iter_mut().find(|(s, _)| *s == section) else {
            return;
        };
        let index = groups
            .iter()
            .position(|g| g.heading == heading)
            .unwrap_or_else(|| {
                groups.push(Group {
                    heading,
                    rows: Vec::new(),
                });
                groups.len() - 1
            });
        let group = &mut groups[index];
        match group.rows.iter_mut().find(|(_, l)| *l == label) {
            Some((keys, _)) if keys.split('/').any(|k| k == key) => {}
            Some((keys, _)) => {
                keys.push('/');
                keys.push_str(key);
            }
            None => group.rows.push((key.to_owned(), label)),
        }
    };
    for binding in BINDINGS {
        let (section, heading) = place(binding);
        add(section, heading, binding.key, binding.label);
    }
    for (keys, label) in EDITOR_KEYS {
        add(Section::Prompts, Some("Typing, in any prompt"), keys, label);
    }
    sections.retain(|(_, groups)| !groups.is_empty());
    sections
}

const NOTE: &str = "Scripts and agents: use the `ms-todo` commands.";

/// One row of the help screen as drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HelpLine {
    Title(String),
    Heading(String),
    /// The keys padded to the column's key width (blank on a label's
    /// wrapped lines), then the label.
    Row {
        keys: String,
        label: String,
    },
    Note(String),
    Blank,
}

/// The help screen for a terminal of one size: its box and each column's
/// lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page {
    /// The box, borders included.
    pub width: u16,
    pub height: u16,
    /// Each column's width and lines.
    pub columns: Vec<(u16, Vec<HelpLine>)>,
    /// Rows of the longest column.
    pub rows: u16,
    /// Rows that fit in the box.
    pub visible: u16,
}

impl Page {
    /// How far down it scrolls: the last row at the bottom of the box.
    pub fn max_scroll(&self) -> u16 {
        self.rows.saturating_sub(self.visible)
    }

    /// PgDn's step: a screenful, keeping one row for context.
    pub fn page(&self) -> u16 {
        self.visible.saturating_sub(1).max(1)
    }
}

/// Between the two columns.
const GAP: u16 = 2;
/// Before a heading, before a row's keys, and between the keys and the
/// label; a section's title is one in.
pub const HEADING_INDENT: usize = 2;
pub const INDENT: usize = 3;
pub const SPACER: usize = 2;

/// The layout for a terminal `screen` in size: two columns if both fit
/// side by side unwrapped, else one, wrapped to the width.
pub fn layout(screen: Size) -> Page {
    let mut parts: Vec<Part> = content()
        .into_iter()
        .flat_map(|(section, groups)| parts(section, groups))
        .collect();
    if let Some(last) = parts.last_mut() {
        last.items.push(Item::Blank);
        last.items.push(Item::Note(NOTE));
    }
    // The box keeps the title bar and hint bar in view when there's room.
    let max_height = if screen.height >= 8 {
        screen.height - 2
    } else {
        screen.height
    };
    let split = best_split(&parts);
    let (left, right) = parts.split_at(split);
    let (left, right) = (join(left), join(right));
    let widths = [natural_width(&left), natural_width(&right)];
    let two_wide = widths[0] + widths[1] + usize::from(GAP) + 2;
    let columns: Vec<(u16, Vec<HelpLine>)> =
        if !right.is_empty() && two_wide <= usize::from(screen.width) {
            [left, right]
                .into_iter()
                .zip(widths)
                .map(|(items, width)| (to_u16(width), lines(&items, width)))
                .collect()
        } else {
            let all = join(&parts);
            let width = natural_width(&all).min(usize::from(screen.width.saturating_sub(2)));
            vec![(to_u16(width), lines(&all, width))]
        };
    let rows = to_u16(columns.iter().map(|(_, l)| l.len()).max().unwrap_or(0));
    let visible = rows.min(max_height.saturating_sub(2));
    let inner: u16 = columns.iter().map(|(w, _)| *w).sum::<u16>()
        + GAP * to_u16(columns.len().saturating_sub(1));
    Page {
        width: (inner + 2).min(screen.width),
        height: visible + 2,
        columns,
        rows,
        visible,
    }
}

/// What a column is made of before it's fitted to a width.
#[derive(Clone, Debug)]
enum Item {
    /// A section's title; `true` when it's carried on into the next
    /// column.
    Title(&'static str, bool),
    Heading(&'static str),
    Row(String, &'static str),
    Note(&'static str),
    Blank,
}

/// One heading's rows, the unit the columns are split at.
#[derive(Clone, Debug)]
struct Part {
    section: Section,
    /// The section's first part, under its title.
    first: bool,
    items: Vec<Item>,
}

fn parts(section: Section, groups: Vec<Group>) -> Vec<Part> {
    groups
        .into_iter()
        .enumerate()
        .map(|(index, group)| {
            let heading = group.heading.map(Item::Heading);
            let rows = group
                .rows
                .into_iter()
                .map(|(keys, label)| Item::Row(keys, label));
            Part {
                section,
                first: index == 0,
                items: heading.into_iter().chain(rows).collect(),
            }
        })
        .collect()
}

/// Parts one after another, each section under its title with a blank
/// row before it; a column that starts partway through a section repeats
/// its title, marked as continued.
fn join(parts: &[Part]) -> Vec<Item> {
    let mut all = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        if part.first || index == 0 {
            if index > 0 {
                all.push(Item::Blank);
            }
            all.push(Item::Title(part.section.title(), !part.first));
        }
        all.extend(part.items.iter().cloned());
    }
    all
}

/// Where to break the parts into two columns, keeping their order, so
/// the taller column is as short as it can be.
fn best_split(parts: &[Part]) -> usize {
    let height = |parts: &[Part]| join(parts).len();
    (1..parts.len())
        .min_by_key(|&at| height(&parts[..at]).max(height(&parts[at..])))
        .unwrap_or(parts.len())
}

fn key_width(items: &[Item]) -> usize {
    items
        .iter()
        .filter_map(|item| match item {
            Item::Row(keys, _) => Some(keys.width()),
            _ => None,
        })
        .max()
        .unwrap_or(0)
}

/// The width a column needs to show every item on one line, with a
/// column of space on the right.
fn natural_width(items: &[Item]) -> usize {
    let keys = key_width(items);
    items
        .iter()
        .map(|item| match item {
            Item::Title(text, continued) => 1 + title(text, *continued).width(),
            Item::Note(text) => 1 + text.width(),
            Item::Heading(text) => HEADING_INDENT + text.width(),
            Item::Row(_, label) => INDENT + keys + SPACER + label.width(),
            Item::Blank => 0,
        })
        .max()
        .unwrap_or(0)
        + 1
}

/// The items as lines `width` wide, wrapping what's wider.
fn lines(items: &[Item], width: usize) -> Vec<HelpLine> {
    let keys_width = key_width(items);
    let text = |indent: usize, text: &str| wrap(text, width.saturating_sub(indent + 1));
    let mut lines = Vec::new();
    for item in items {
        match item {
            Item::Title(name, continued) => lines.extend(
                text(1, &title(name, *continued))
                    .into_iter()
                    .map(HelpLine::Title),
            ),
            Item::Heading(heading) => {
                lines.extend(
                    text(HEADING_INDENT, heading)
                        .into_iter()
                        .map(HelpLine::Heading),
                );
            }
            Item::Note(note) => lines.extend(text(1, note).into_iter().map(HelpLine::Note)),
            Item::Blank => lines.push(HelpLine::Blank),
            Item::Row(keys, label) => {
                let wrapped = text(INDENT + keys_width + SPACER, label);
                for (index, label) in wrapped.into_iter().enumerate() {
                    let keys = if index == 0 { keys.as_str() } else { "" };
                    let pad = keys_width.saturating_sub(keys.width());
                    lines.push(HelpLine::Row {
                        keys: format!("{keys}{}", " ".repeat(pad)),
                        label,
                    });
                }
            }
        }
    }
    lines
}

fn title(name: &str, continued: bool) -> String {
    if continued {
        format!("{name} (continued)")
    } else {
        name.to_owned()
    }
}

/// Word-wrap `text` to `width` columns, breaking a word only when it's
/// wider than a line on its own.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split(' ') {
        let mut word = word;
        loop {
            let needed = if line.is_empty() {
                word.width()
            } else {
                line.width() + 1 + word.width()
            };
            if needed <= width {
                if !line.is_empty() {
                    line.push(' ');
                }
                line.push_str(word);
                break;
            }
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
                continue;
            }
            let (head, tail) = split_at_width(word, width);
            lines.push(head.to_owned());
            word = tail;
        }
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

/// The longest start of `word` at most `width` wide, at least one
/// character, and the rest.
fn split_at_width(word: &str, width: usize) -> (&str, &str) {
    let mut used = 0;
    for (index, ch) in word.char_indices() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if index > 0 && used + ch_width > width {
            return word.split_at(index);
        }
        used += ch_width;
    }
    (word, "")
}

fn to_u16(n: usize) -> u16 {
    u16::try_from(n).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<(String, &'static str)> {
        content()
            .into_iter()
            .flat_map(|(_, groups)| groups)
            .flat_map(|group| group.rows)
            .collect()
    }

    /// A key added to the registry can never be missing from help.
    #[test]
    fn every_binding_is_in_help() {
        let rows = rows();
        for binding in BINDINGS {
            assert!(
                rows.iter().any(|(keys, label)| *label == binding.label
                    && (keys == binding.key || keys.split('/').any(|key| key == binding.key))),
                "{} ({}) isn't in help",
                binding.key,
                binding.label
            );
        }
        for (keys, label) in EDITOR_KEYS {
            assert!(rows.contains(&((*keys).to_owned(), *label)), "{keys}");
        }
    }

    #[test]
    fn keys_are_grouped_by_where_they_are_pressed() {
        let sections = content();
        let titles: Vec<_> = sections.iter().map(|(s, _)| s.title()).collect();
        assert_eq!(
            titles,
            [
                "Navigation",
                "Tasks",
                "Detail pane",
                "Prompts and editing",
                "Views and palette"
            ]
        );
        let group = |section: Section, heading: Option<&str>| {
            sections
                .iter()
                .find(|(s, _)| *s == section)
                .and_then(|(_, groups)| groups.iter().find(|g| g.heading == heading))
                .map(|g| g.rows.clone())
                .unwrap_or_default()
        };
        let navigation = group(Section::Navigation, None);
        assert!(navigation.contains(&("j/Down".into(), "Down")));
        assert!(navigation.contains(&("q/Ctrl-c".into(), "Quit")));
        assert!(group(Section::Tasks, None).contains(&("t".into(), "My Day")));
        assert!(group(Section::Tasks, None).contains(&("e".into(), "Edit")));
        assert!(group(Section::Detail, None).contains(&("e".into(), "Edit field")));
        assert!(group(Section::Detail, None).contains(&("Enter".into(), "Edit this field")));
        // Esc and Ctrl-c cancel every prompt: one row, not four.
        assert_eq!(
            group(Section::Prompts, Some("Any prompt"))
                .iter()
                .filter(|(_, label)| *label == "Cancel")
                .count(),
            1
        );
        assert!(group(Section::Views, Some("This help")).contains(&("Esc/?/q".into(), "Close")));
    }

    #[test]
    fn one_column_when_narrow_and_two_when_wide() {
        let narrow = layout(Size::new(80, 24));
        assert_eq!(narrow.columns.len(), 1);
        assert_eq!(narrow.height, 22, "the whole height but the two bars");
        assert!(narrow.max_scroll() > 0, "help is taller than 24 rows");
        let wide = layout(Size::new(140, 40));
        assert_eq!(wide.columns.len(), 2);
        assert!(wide.width <= 140);
    }

    /// However small the terminal, every label is on some line, whole:
    /// wrapped, never cut.
    #[test]
    fn nothing_is_cut_off_at_any_width() {
        for width in [30, 50, 80, 100, 140, 200] {
            let page = layout(Size::new(width, 24));
            let text: String = page
                .columns
                .iter()
                .flat_map(|(_, lines)| lines)
                .map(|line| match line {
                    HelpLine::Row { label, .. }
                    | HelpLine::Title(label)
                    | HelpLine::Heading(label)
                    | HelpLine::Note(label) => format!("{label} "),
                    HelpLine::Blank => String::new(),
                })
                .collect();
            // A word too long for a narrow column is broken, not dropped.
            let squeezed = |s: &str| s.split_whitespace().collect::<String>();
            let all = squeezed(&text);
            for (_, label) in rows() {
                assert!(all.contains(&squeezed(label)), "{label} at {width}");
            }
            for (column_width, lines) in &page.columns {
                for line in lines {
                    let drawn = match line {
                        HelpLine::Row { keys, label } => {
                            INDENT + keys.width() + SPACER + label.width()
                        }
                        HelpLine::Title(t) | HelpLine::Note(t) => 1 + t.width(),
                        HelpLine::Heading(t) => HEADING_INDENT + t.width(),
                        HelpLine::Blank => 0,
                    };
                    assert!(drawn <= usize::from(*column_width), "{line:?} at {width}");
                }
            }
        }
    }

    #[test]
    fn wrap_breaks_between_words() {
        assert_eq!(wrap("one two three", 7), ["one two", "three"]);
        assert_eq!(wrap("abcdefgh", 3), ["abc", "def", "gh"]);
        assert_eq!(wrap("", 5), [""]);
    }
}
