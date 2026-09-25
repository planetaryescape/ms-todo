use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::text::Line;
use ratatui::widgets::{Clear, List, ListItem, ListState, Paragraph};

use super::{centered, pane, selection};
use crate::app::App;
use crate::theme::{BUILTIN, Capability};

/// The theme picker: every built-in theme, the one under the cursor drawn
/// on the whole screen behind it.
pub fn draw(frame: &mut Frame, app: &App, index: usize) {
    let theme = &app.theme;
    // The note says why the preview doesn't change with NO_COLOR set.
    let note = match app.theme_choice.capability {
        Capability::Monochrome => "NO_COLOR is set, so no theme adds colour",
        _ => "Enter keeps it in config.toml; Esc goes back",
    };
    let rows = u16::try_from(BUILTIN.len()).unwrap_or(u16::MAX);
    let area = centered(frame.area(), 46, rows + 4);
    frame.render_widget(Clear, area);
    let block = pane(theme, " Theme ", true);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [list, _, footer] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    let items: Vec<ListItem> = BUILTIN
        .iter()
        .map(|builtin| ListItem::new(Line::raw(builtin.name)))
        .collect();
    let mut state = ListState::default().with_selected(Some(index));
    frame.render_stateful_widget(
        List::new(items).highlight_style(selection(theme, true)),
        list,
        &mut state,
    );
    frame.render_widget(Paragraph::new(Line::styled(note, theme.text_dim)), footer);
}
