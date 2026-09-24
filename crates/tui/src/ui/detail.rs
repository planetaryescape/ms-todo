use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use super::task_list::{due_label, sync_marker};
use super::{AMBER, DIM, ERROR, focused, pane};
use crate::app::{App, Pane, SyncMarker};

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let block = pane(" Detail ".into(), focused(app, Pane::Detail));
    let Some(task) = app.selected() else {
        frame.render_widget(block, area);
        return;
    };
    let label = |name: &'static str| Span::styled(format!("{name:<11}"), Style::default().fg(DIM));
    let field = |name: &'static str, value: String| Line::from(vec![label(name), Span::raw(value)]);
    let mut lines = vec![
        Line::from(Span::styled(
            task.title.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::default(),
        field(
            "Status",
            if task.completed { "completed" } else { "open" }.into(),
        ),
    ];
    if let Some(name) = app.list_name(&task.list_id) {
        lines.push(field("List", name.to_owned()));
    }
    if let Some(due) = task.due {
        let mut shown = due.format("%a %-d %b %Y").to_string();
        let today = app.clock.today;
        if due < today && !task.completed {
            shown.push_str(" (overdue)");
        } else if due.signed_duration_since(today).num_days().abs() <= 1 {
            shown = format!("{shown} ({})", due_label(due, today));
        }
        lines.push(field("Due", shown));
    }
    if let Some(reminder) = task.reminder {
        lines.push(field(
            "Reminder",
            reminder.format("%a %-d %b %Y %H:%M").to_string(),
        ));
    }
    if task.important {
        lines.push(field("Importance", "high".into()));
    }
    if let Some(recurrence) = &task.recurrence {
        lines.push(field("Repeats", recurrence.clone()));
    }
    if let Some((checked, total)) = task.steps {
        lines.push(field("Steps", format!("{checked} of {total} done")));
    }
    if !task.categories.is_empty() {
        lines.push(field("Categories", task.categories.join(", ")));
    }
    let (state, style) = match task.sync {
        SyncMarker::Synced => ("synced", Style::default()),
        SyncMarker::Pending => (
            "pending: waiting to reach Microsoft To Do",
            Style::default().fg(DIM),
        ),
        SyncMarker::Unknown => (
            "outcome unknown: resolve it with `ms-todo outbox list`",
            Style::default().fg(AMBER),
        ),
        SyncMarker::Failed => (
            "failed: Microsoft To Do rejected a change",
            Style::default().fg(ERROR),
        ),
    };
    lines.push(Line::from(vec![
        label("Sync"),
        sync_marker(task.sync, &app.glyphs),
        Span::raw(if task.sync == SyncMarker::Synced {
            ""
        } else {
            " "
        }),
        Span::styled(state, style),
    ]));
    if let Some(why) = app.rejections.get(&task.id) {
        lines.push(Line::from(vec![
            label(""),
            Span::styled(why.clone(), Style::default().fg(ERROR)),
        ]));
    }
    if let Some(notes) = task.notes() {
        lines.push(Line::default());
        lines.extend(notes.lines().map(|line| Line::raw(line.to_owned())));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}
