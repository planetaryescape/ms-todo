use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem, ListState, Paragraph};

use super::{ACCENT, DIM, centered, line_input, pane, selection};
use crate::app::App;
use crate::app::line_editor::LineEditor;

/// The most matches shown at once; the list scrolls past them.
const ROWS: u16 = 12;

/// The command palette: the query, then the matches, best first, each with
/// the keys that do the same.
pub fn draw(frame: &mut Frame, app: &App, query: &LineEditor, index: usize) {
    let items = app.palette_items(&query.text());
    let rows = u16::try_from(items.len()).unwrap_or(ROWS).clamp(1, ROWS);
    let area = centered(frame.area(), 60, rows + 4);
    frame.render_widget(Clear, area);
    let block = pane(" Palette ".into(), true);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [input, _, list] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new(Line::from(
            [Span::styled(": ", Style::default().fg(ACCENT))]
                .into_iter()
                .chain(line_input::single(query, Style::default(), &app.glyphs))
                .collect::<Vec<_>>(),
        )),
        input,
    );
    if items.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "No command or list matches",
                Style::default().fg(DIM),
            )),
            list,
        );
        return;
    }
    let width = usize::from(list.width);
    let rows: Vec<ListItem> = items
        .iter()
        .map(|item| {
            let gap = width
                .saturating_sub(item.label.chars().count() + item.keys.chars().count())
                .max(1);
            ListItem::new(Line::from(vec![
                Span::raw(item.label.clone()),
                Span::raw(" ".repeat(gap)),
                Span::styled(
                    item.keys.clone(),
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                ),
            ]))
        })
        .collect();
    let mut state = ListState::default().with_selected(Some(index));
    frame.render_stateful_widget(
        List::new(rows).highlight_style(selection(true)),
        list,
        &mut state,
    );
}
