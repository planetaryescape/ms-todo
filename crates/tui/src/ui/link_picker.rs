use ms_todo_core::links::{Link, openable};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem, ListState, Paragraph};

use super::{centered, pane, selection};
use crate::app::App;

/// A task's links: each one's text and URL, one that won't open marked.
pub fn draw(frame: &mut Frame, app: &App, links: &[Link], index: usize) {
    let theme = &app.theme;
    let rows = u16::try_from(links.len()).unwrap_or(u16::MAX).min(12);
    let area = centered(frame.area(), 76, rows + 4);
    frame.render_widget(Clear, area);
    let block = pane(theme, " Links ", true);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [list, _, footer] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    let items: Vec<ListItem> = links
        .iter()
        .map(|link| {
            let mut spans = vec![
                Span::styled(link.text.clone(), theme.text),
                Span::raw("  "),
                Span::styled(link.url.clone(), theme.link),
            ];
            if openable(&link.url).is_err() {
                spans.push(Span::styled("  (won't open)", theme.warning));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    let mut state = ListState::default().with_selected(Some(index));
    frame.render_stateful_widget(
        List::new(items).highlight_style(selection(theme, true)),
        list,
        &mut state,
    );
    frame.render_widget(
        Paragraph::new(Line::styled(
            "Linked resources first, then the notes",
            theme.text_dim,
        )),
        footer,
    );
}
