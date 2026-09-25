// Adapted from mxr crates/tui/src/ui/hint_bar.rs @ dfb23d10138b1cfc24f8ea7450d3426e5e4da37a:
// contextual hints from the keybinding registry. Changes: a prompt or the
// delete confirmation takes the bar over, since that's where the typing
// is; the sync state is in the title bar instead.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{ACCENT, DIM, ERROR, line_input};
use crate::action::Action;
use crate::app::edit::Field;
use crate::app::line_editor::LineEditor;
use crate::app::{App, Mode};
use crate::keybindings::{Context, hints, key_for};

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let glyphs = &app.glyphs;
    let hint_spans = |context| {
        let mut spans = Vec::new();
        for (keys, label) in hints(context) {
            spans.push(Span::styled(
                format!(" {keys} "),
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(format!("{label} "), Style::default().fg(DIM)));
        }
        spans
    };
    let typed = |input: &LineEditor| line_input::single(input, Style::default(), glyphs);
    let key = |keys: &str| {
        Span::styled(
            keys.to_owned(),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        )
    };
    let left = match &app.mode {
        Mode::Adding { input } => {
            let list = match &app.shown {
                Some(ms_todo_protocol::Scope::List { id }) => {
                    app.list_name(id).unwrap_or("Tasks").to_owned()
                }
                shown => format!("Tasks, from {}", app.scope_name(shown.as_ref())),
            };
            let mut spans = vec![Span::styled(
                format!(" Add to {list}: "),
                Style::default().fg(ACCENT),
            )];
            spans.extend(typed(input));
            Line::from(spans)
        }
        Mode::Filtering { input } => {
            let mut spans = vec![Span::styled(" / ", Style::default().fg(ACCENT))];
            spans.extend(typed(input));
            if let Some(error) = &app.filter_error {
                spans.push(Span::styled(
                    format!("  {error}"),
                    Style::default().fg(ERROR),
                ));
            }
            Line::from(spans)
        }
        Mode::Editing { field, .. } => {
            let mut spans = vec![Span::styled(
                format!(" Edit {}: ", field.name().to_lowercase()),
                Style::default().fg(ACCENT),
            )];
            spans.extend(hint_spans(app.context()));
            spans.push(key(&format!(" {} ", glyphs.left_right)));
            spans.push(Span::styled("Move ", Style::default().fg(DIM)));
            Line::from(spans)
        }
        // The one-line field picker: each field with its key in brackets,
        // `[t]itle`, then the picker's other keys.
        Mode::ChoosingField { .. } => {
            let mut spans = vec![Span::styled(" edit: ", Style::default().fg(ACCENT))];
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
        Mode::ChoosingImportance { .. } => {
            let mut spans = vec![Span::styled(" importance: ", Style::default().fg(ACCENT))];
            spans.extend(hint_spans(Context::Importance));
            Line::from(spans)
        }
        Mode::ConfirmDelete { what, .. } => {
            let mut spans = vec![Span::styled(
                format!(" Delete {what}? "),
                Style::default().fg(ERROR).add_modifier(Modifier::BOLD),
            )];
            spans.extend(hint_spans(app.context()));
            Line::from(spans)
        }
        _ => Line::from(hint_spans(app.context())),
    };
    frame.render_widget(Paragraph::new(left), area);
}
