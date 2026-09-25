use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState};

use super::{DIM, focused, pane, selection};
use crate::app::scope::{Entry, view_name};
use crate::app::{App, Pane};
use ms_todo_protocol::Scope;

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let has_focus = focused(app, Pane::Sidebar);
    let glyphs = &app.glyphs;
    // Inside the borders.
    let width = usize::from(area.width.saturating_sub(2));
    let items: Vec<ListItem> = app
        .entries()
        .iter()
        .map(|entry| {
            let (icon, name, count, heading) = match entry {
                Entry::View(scope) => {
                    let (icon, count) = match scope {
                        Scope::Important => (glyphs.important, app.counts.important),
                        Scope::Planned => (glyphs.planned, app.counts.planned),
                        Scope::All => (glyphs.all, app.counts.all),
                        _ => (glyphs.completed, app.counts.completed),
                    };
                    (icon.to_owned(), view_name(scope).to_owned(), count, false)
                }
                Entry::Folder {
                    name,
                    collapsed,
                    count,
                } => {
                    let icon = if *collapsed {
                        glyphs.folder_closed
                    } else {
                        glyphs.folder_open
                    };
                    (icon.to_owned(), name.clone(), *count, true)
                }
                Entry::List {
                    id,
                    name,
                    in_folder,
                } => {
                    // A list in a folder is drawn under its heading.
                    let indent = if *in_folder { "  " } else { "" };
                    (
                        format!("{indent}{}", glyphs.list),
                        name.clone(),
                        app.counts.lists.get(id).copied().unwrap_or(0),
                        false,
                    )
                }
            };
            let count = if count == 0 {
                String::new()
            } else {
                count.to_string()
            };
            let label = format!("{icon} {name}");
            let room = width.saturating_sub(count.chars().count() + 1);
            let label: String = label.chars().take(room).collect();
            let gap = width.saturating_sub(label.chars().count() + count.chars().count());
            let label_style = if heading {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(vec![
                Span::styled(label, label_style),
                Span::raw(" ".repeat(gap)),
                Span::styled(count, Style::default().fg(DIM)),
            ]))
        })
        .collect();
    let mut state = ListState::default().with_selected(Some(app.sidebar_index));
    let list = List::new(items)
        .block(pane(" Lists ".into(), has_focus))
        .highlight_style(selection(has_focus));
    frame.render_stateful_widget(list, area, &mut state);
}
