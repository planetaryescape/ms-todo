// Adapted from mxr crates/tui/src/ui/hint_bar.rs @ dfb23d10138b1cfc24f8ea7450d3426e5e4da37a:
// contextual hints from the keybinding registry. Changes: a prompt or the
// delete confirmation takes the bar over, since that's where the typing
// is (quick add types in its modal instead); the sync state is in the
// title bar instead.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::line_input;
use crate::action::Action;
use crate::app::edit::Field;
use crate::app::line_editor::LineEditor;
use crate::app::{App, Mode};
use crate::keybindings::{Context, hints, key_for};

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let glyphs = &app.glyphs;
    let theme = &app.theme;
    let hint_spans = |context| {
        let mut spans = Vec::new();
        for (keys, label) in hints(context) {
            spans.push(Span::styled(format!(" {keys} "), theme.key));
            spans.push(Span::styled(format!("{label} "), theme.text_dim));
        }
        spans
    };
    let typed = |input: &LineEditor| line_input::single(input, Style::default(), glyphs, theme);
    let key = |keys: &str| Span::styled(keys.to_owned(), theme.key);
    let left = match &app.mode {
        Mode::Filtering { input } => {
            let mut spans = vec![Span::styled(" / ", theme.accent)];
            spans.extend(typed(input));
            if let Some(error) = &app.filter_error {
                spans.push(Span::styled(format!("  {error}"), theme.error));
            }
            Line::from(spans)
        }
        Mode::Editing { field, .. } => {
            let mut spans = vec![Span::styled(
                format!(" Edit {}: ", field.name().to_lowercase()),
                theme.accent,
            )];
            spans.extend(hint_spans(app.context()));
            spans.push(key(&format!(" {} ", glyphs.left_right)));
            spans.push(Span::styled("Move ", theme.text_dim));
            Line::from(spans)
        }
        // The one-line field picker: each field with its key in brackets,
        // `[t]itle`, then the picker's other keys.
        Mode::ChoosingField { .. } => {
            let mut spans = vec![Span::styled(" edit: ", theme.accent)];
            for field in Field::ALL {
                let name = field.name().to_lowercase();
                let Some(bound) = key_for(Context::Fields, Action::EditField(field)) else {
                    continue;
                };
                let (shown, rest) = match name.strip_prefix(bound) {
                    Some(rest) => (format!("[{bound}]"), rest.to_owned()),
                    None => (format!("[{bound}]"), format!(" {name}")),
                };
                spans.push(key(&shown));
                spans.push(Span::raw(format!("{rest} ")));
            }
            spans.extend(hint_spans(Context::Fields));
            Line::from(spans)
        }
        // The folder prompt: what's typed, then the folders it could be.
        Mode::MovingList { list_id, input } => {
            let name = app.list_name(list_id).unwrap_or("list");
            let mut spans = vec![Span::styled(
                format!(" Move {name} to folder: "),
                theme.accent,
            )];
            spans.extend(typed(input));
            let suggestions = app.folder_suggestions(&input.text());
            if !suggestions.is_empty() {
                spans.push(Span::styled(
                    format!("  {} ", suggestions.join(" \u{b7} ")),
                    theme.text_dim,
                ));
            }
            spans.extend(hint_spans(Context::Folder));
            spans.push(Span::styled(" Enter on empty: no folder ", theme.text_dim));
            Line::from(spans)
        }
        // One due date for several tasks: what's typed, then what it
        // resolves to or why it can't be sent.
        Mode::SettingDue {
            what, input, error, ..
        } => {
            let mut spans = vec![Span::styled(
                format!(" Due date for {what}: "),
                theme.accent,
            )];
            spans.extend(typed(input));
            let under = match (error.clone(), app.date_preview()) {
                (Some(error), _) | (None, Some(Err(error))) => {
                    Span::styled(format!("  {error} "), theme.error)
                }
                (None, Some(Ok(preview))) => Span::styled(format!("  {preview} "), theme.text_dim),
                (None, None) => Span::raw(""),
            };
            spans.push(under);
            spans.extend(hint_spans(Context::Prompt));
            Line::from(spans)
        }
        // Who several tasks wait on: what's typed, then what empty does.
        Mode::Assigning { what, input, .. } => {
            let mut spans = vec![Span::styled(format!(" Assign {what} to: "), theme.accent)];
            spans.extend(typed(input));
            spans.extend(hint_spans(Context::Prompt));
            spans.push(Span::styled(" Enter on empty: no one ", theme.text_dim));
            Line::from(spans)
        }
        Mode::ChoosingImportance { .. } => {
            let mut spans = vec![Span::styled(" importance: ", theme.accent)];
            spans.extend(hint_spans(Context::Importance));
            Line::from(spans)
        }
        Mode::EditingChild { target, .. } => {
            let mut spans = vec![Span::styled(format!(" {}: ", target.label()), theme.accent)];
            spans.extend(hint_spans(app.context()));
            spans.push(key(&format!(" {} ", glyphs.left_right)));
            spans.push(Span::styled("Move ", theme.text_dim));
            Line::from(spans)
        }
        Mode::Attaching { .. } => {
            let mut spans = vec![Span::styled(" Attach the file at: ", theme.accent)];
            spans.extend(hint_spans(app.context()));
            spans.push(key(&format!(" {} ", glyphs.left_right)));
            spans.push(Span::styled("Move ", theme.text_dim));
            Line::from(spans)
        }
        Mode::ConfirmDelete { what, .. } | Mode::ConfirmDeleteChild { what, .. } => {
            let mut spans = vec![Span::styled(
                format!(" Delete {what}? "),
                theme.banner_error,
            )];
            spans.extend(hint_spans(app.context()));
            Line::from(spans)
        }
        _ => Line::from(hint_spans(app.context())),
    };
    frame.render_widget(Paragraph::new(left), area);
}
