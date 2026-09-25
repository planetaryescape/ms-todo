// The split into pure render functions per region is adapted from mxr
// crates/tui/src/ui/ @ dfb23d10138b1cfc24f8ea7450d3426e5e4da37a (sidebar, list,
// detail, status bar, hint bar, help modal). Changes: each function takes
// the whole `App` read-only and nothing else, and the layout is 08's three
// panes.

//! Drawing: pure functions of the [`App`]. Nothing here changes state or
//! does I/O, so a frame is the same for the same state.

mod add_modal;
mod detail;
mod diagnostics;
mod hint_bar;
mod line_input;
mod link_picker;
mod modals;
mod move_picker;
mod onboarding;
mod palette;
mod sidebar;
mod status_line;
mod task_list;
mod theme_picker;
mod title_bar;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Borders};

use crate::app::{App, Mode, Pane};
use crate::theme::Theme;

pub fn draw(frame: &mut Frame, app: &App) {
    // The theme's background and text colour under everything; nothing
    // for the terminal theme, whose colours are the terminal's own.
    let area = frame.area();
    if app.theme.base != Style::default() {
        frame.buffer_mut().set_style(area, app.theme.base);
    }
    let [title, main, status, hints] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);
    let [side, list, detail] = Layout::horizontal([
        Constraint::Length(24),
        Constraint::Fill(3),
        Constraint::Fill(2),
    ])
    .areas(main);
    title_bar::draw(frame, title, app);
    // The diagnostics page covers the panes, so they aren't drawn under it.
    if app.mode != Mode::Diagnostics {
        if app.sign_in_required
            && let Some(command) = &app.sign_in_command
        {
            onboarding::draw(frame, main, app, command);
        } else {
            sidebar::draw(frame, side, app);
            task_list::draw(frame, list, app);
            detail::draw(frame, detail, app);
        }
    }
    status_line::draw(frame, status, app);
    hint_bar::draw(frame, hints, app);
    match &app.mode {
        Mode::Help { scroll } => modals::help(frame, app, *scroll),
        Mode::Adding { input, parsed, .. } => add_modal::draw(frame, app, input, parsed.as_ref()),
        Mode::Picker {
            candidates, index, ..
        } => modals::picker(frame, app, candidates, *index),
        Mode::Palette { query, index } => palette::draw(frame, app, query, *index),
        Mode::MovingTasks {
            ids,
            what,
            query,
            index,
        } => move_picker::draw(frame, app, ids, what, query, *index),
        Mode::Diagnostics => diagnostics::draw(frame, main, app),
        Mode::Themes { index, .. } => theme_picker::draw(frame, app, *index),
        Mode::Links { links, index } => link_picker::draw(frame, app, links, *index),
        _ => {}
    }
}

/// A pane's frame, highlighted when it has the focus. It carries the
/// theme's background, so a modal drawn over `Clear` keeps it.
fn pane(theme: &Theme, title: impl Into<Line<'static>>, focused: bool) -> Block<'static> {
    let (border, title_style) = if focused {
        (theme.border_focused, theme.title)
    } else {
        (theme.border, theme.title_unfocused)
    };
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .style(theme.base)
        .border_style(border)
        .title(title)
        .title_style(title_style)
}

fn focused(app: &App, pane: Pane) -> bool {
    app.mode == Mode::Normal && app.focus == pane
}

/// The selected row's style: the theme's selection in the focused pane,
/// bold elsewhere.
fn selection(theme: &Theme, focused: bool) -> Style {
    if focused {
        theme.selection
    } else {
        theme.selection_unfocused
    }
}

/// A rectangle `width` by `height` in the middle of `area`, clipped to it.
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

#[cfg(test)]
mod tests;
