// The split into pure render functions per region is adapted from mxr
// crates/tui/src/ui/ @ dfb23d10138b1cfc24f8ea7450d3426e5e4da37a (sidebar, list,
// detail, status bar, hint bar, help modal). Changes: each function takes
// the whole `App` read-only and nothing else, and the layout is 08's three
// panes.

//! Drawing: pure functions of the [`App`]. Nothing here changes state or
//! does I/O, so a frame is the same for the same state.

mod detail;
mod hint_bar;
mod modals;
mod sidebar;
mod status_line;
mod task_list;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders};

use crate::app::{App, Mode, Pane};

const ACCENT: Color = Color::Cyan;
const DIM: Color = Color::DarkGray;
const AMBER: Color = Color::Yellow;
const ERROR: Color = Color::Red;

pub fn draw(frame: &mut Frame, app: &App) {
    let [main, status, hints] = Layout::vertical([
        Constraint::Min(3),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    let [side, list, detail] = Layout::horizontal([
        Constraint::Length(24),
        Constraint::Fill(3),
        Constraint::Fill(2),
    ])
    .areas(main);
    sidebar::draw(frame, side, app);
    task_list::draw(frame, list, app);
    detail::draw(frame, detail, app);
    status_line::draw(frame, status, app);
    hint_bar::draw(frame, hints, app);
    match &app.mode {
        Mode::Help => modals::help(frame),
        Mode::Picker {
            candidates, index, ..
        } => modals::picker(frame, app, candidates, *index),
        _ => {}
    }
}

/// A pane's frame, highlighted when it has the focus.
fn pane(title: String, focused: bool) -> Block<'static> {
    let style = if focused {
        Style::default().fg(ACCENT)
    } else {
        Style::default().fg(DIM)
    };
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(style)
        .title(title)
        .title_style(style.add_modifier(Modifier::BOLD))
}

fn focused(app: &App, pane: Pane) -> bool {
    app.mode == Mode::Normal && app.focus == pane
}

/// The selected row's style: reversed in the focused pane, bold elsewhere.
fn selection(focused: bool) -> Style {
    if focused {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default().add_modifier(Modifier::BOLD)
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
