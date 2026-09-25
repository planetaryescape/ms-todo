use ms_todo_protocol::Candidate;
use ratatui::Frame;
use ratatui::layout::{Margin, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Clear, List, ListItem, ListState, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
    Wrap,
};

use super::{centered, pane, selection};
use crate::app::App;
use crate::help::{self, HelpLine};
use crate::theme::Theme;

/// Every key, from the registry, then the line editor's: in one column or
/// two, scrolled, with a scrollbar and "more" on the border whenever rows
/// are out of view.
pub fn help(frame: &mut Frame, app: &App, scroll: u16) {
    let theme = &app.theme;
    let page = help::layout(frame.area().as_size());
    // A resize the app hasn't heard of yet can't scroll past the end.
    let max = page.max_scroll();
    let scroll = scroll.min(max);
    let area = centered(frame.area(), page.width, page.height);
    frame.render_widget(Clear, area);
    let mut block = pane(
        theme,
        format!(" ms-todo {} \u{b7} Keys ", app.version),
        true,
    );
    let more = if scroll < max {
        Some(app.glyphs.more_below)
    } else if scroll > 0 {
        Some(app.glyphs.more_above)
    } else {
        None
    };
    if let Some(more) = more {
        block = block.title_bottom(Line::styled(format!(" {more} "), theme.accent).right_aligned());
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let mut x = inner.x;
    for (width, lines) in &page.columns {
        let column = Rect {
            x,
            width: (*width).min(inner.right().saturating_sub(x)),
            ..inner
        };
        let lines: Vec<Line> = lines.iter().map(|line| help_line(theme, line)).collect();
        frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), column);
        x = x.saturating_add(*width).saturating_add(2);
    }
    if max > 0 {
        let mut state = ScrollbarState::new(usize::from(max))
            .position(usize::from(scroll))
            .viewport_content_length(usize::from(page.visible));
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(None)
                .thumb_symbol(app.glyphs.scroll_thumb)
                .thumb_style(theme.accent),
            area.inner(Margin {
                vertical: 1,
                horizontal: 0,
            }),
            &mut state,
        );
    }
}

fn help_line(theme: &Theme, line: &HelpLine) -> Line<'static> {
    match line {
        HelpLine::Title(title) => Line::styled(format!(" {title}"), theme.title),
        HelpLine::Heading(heading) => Line::styled(
            format!("{}{heading}", " ".repeat(help::HEADING_INDENT)),
            theme.text_dim,
        ),
        HelpLine::Row { keys, label } => Line::from(vec![
            Span::styled(
                format!(
                    "{}{keys}{}",
                    " ".repeat(help::INDENT),
                    " ".repeat(help::SPACER)
                ),
                theme.key,
            ),
            Span::styled(label.clone(), theme.text),
        ]),
        HelpLine::Note(note) => Line::styled(format!(" {note}"), theme.text_dim),
        HelpLine::Blank => Line::default(),
    }
}

/// Undoing a recurring completion: Microsoft To Do made a completed copy,
/// and which one to delete is the user's choice, never a guess
/// (docs/blueprint/08-tui.md).
pub fn picker(frame: &mut Frame, app: &App, candidates: &[Candidate], index: usize) {
    let theme = &app.theme;
    let items: Vec<ListItem> = candidates
        .iter()
        .map(|candidate| {
            let list = candidate
                .list_id
                .as_deref()
                .and_then(|id| app.list_name(id))
                .unwrap_or("");
            let created = candidate
                .created_at
                .as_deref()
                .map(|at| at.get(..16).unwrap_or(at).replace('T', " "))
                .unwrap_or_default();
            ListItem::new(Line::from(vec![
                Span::raw(candidate.name.clone()),
                Span::styled(format!("  {list}  created {created}"), theme.text_dim),
            ]))
        })
        .collect();
    let height = u16::try_from(candidates.len() + 4).unwrap_or(u16::MAX);
    let area = centered(frame.area(), 70, height);
    frame.render_widget(Clear, area);
    let block = pane(theme, " Which completed copy to delete? ", true);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(Line::styled(
            "Undo deletes the completed copy you pick:",
            theme.text_dim,
        ))
        .wrap(Wrap { trim: true }),
        ratatui::layout::Rect { height: 1, ..inner },
    );
    let mut state = ListState::default().with_selected(Some(index));
    frame.render_stateful_widget(
        List::new(items).highlight_style(selection(theme, true)),
        ratatui::layout::Rect {
            y: inner.y + 2,
            height: inner.height.saturating_sub(2),
            ..inner
        },
        &mut state,
    );
}
