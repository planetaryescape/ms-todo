use ms_todo_protocol::{DoctorReport, SyncMode, SyncState};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use super::status_line::ago;
use super::{ACCENT, AMBER, DIM, ERROR, pane};
use crate::app::App;
use crate::app::diagnostics::Diagnostics;

/// The diagnostics page, over the three panes: what `ms-todo doctor`
/// reports, from the daemon.
pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let page = &app.diagnostics;
    let title = if page.waiting > 0 {
        " Diagnostics (checking…) "
    } else {
        " Diagnostics "
    };
    frame.render_widget(
        Paragraph::new(lines(app, page))
            .block(pane(title.into(), true))
            .wrap(Wrap { trim: false })
            .scroll((page.scroll, 0)),
        area,
    );
}

fn lines(app: &App, page: &Diagnostics) -> Vec<Line<'static>> {
    let dim = Style::default().fg(DIM);
    let label = |name: &'static str| Span::styled(format!("{name:<12}"), dim);
    let row = |name: &'static str, value: Span<'static>| Line::from(vec![label(name), value]);
    let waiting = || Span::styled("checking…", dim);
    let failed = |why: &str| Span::styled(why.to_owned(), Style::default().fg(ERROR));
    let now = app.clock.unix;
    let mut lines = Vec::new();

    lines.push(row(
        "Signed in",
        match &page.status {
            None => waiting(),
            Some(Ok(status)) if status.signed_in => Span::raw("yes"),
            Some(Ok(_)) => failed("no: run `ms-todo auth login`"),
            Some(Err(_)) => Span::styled("unknown", dim),
        },
    ));
    lines.push(row(
        "Daemon",
        match &page.status {
            None => waiting(),
            Some(Ok(status)) => Span::raw(format!(
                "running: ms-todo {}, pid {}, instance {}, up {}",
                status.version,
                status.pid,
                status.instance,
                ago(now - status.started_at).trim_end_matches(" ago")
            )),
            Some(Err(why)) => failed(why),
        },
    ));
    let report = match &page.report {
        None => {
            lines.push(row("Database", waiting()));
            return lines;
        }
        Some(Err(why)) => {
            lines.push(row("Database", failed(why)));
            lines.extend(problems(page));
            return lines;
        }
        Some(Ok(report)) => report,
    };
    lines.push(row(
        "Database",
        Span::raw(format!(
            "{} ({})",
            report.database_path,
            human_bytes(report.database_bytes)
        )),
    ));
    lines.push(row("Sync", Span::raw(sync_summary(report))));
    let outbox = &report.outbox;
    lines.push(row(
        "Outbox",
        Span::raw(format!(
            "{} pending, {} sending, {} unknown, {} failed, {} done",
            outbox.pending, outbox.inflight, outbox.unknown, outbox.failed, outbox.done
        )),
    ));
    lines.push(row(
        "Flagged",
        if outbox.flagged == 0 {
            Span::raw("none")
        } else {
            Span::styled(
                format!("{} unknown for over a day", outbox.flagged),
                Style::default().fg(AMBER),
            )
        },
    ));
    for op in page.flagged() {
        lines.push(Line::from(vec![
            label(""),
            Span::styled(
                format!(
                    "{} {} \"{}\"",
                    op.op_id,
                    op.action,
                    op.title.as_deref().unwrap_or("")
                ),
                Style::default().fg(AMBER),
            ),
        ]));
    }
    lines.extend(problems(page));

    lines.push(Line::default());
    lines.push(Line::styled(
        format!(
            " {:<24}{:<9}{:<13}{:<14}{}",
            "Scope", "State", "Mode", "Last synced", "Changed"
        ),
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
    ));
    for scope in &report.scopes {
        let name = match (&scope.list_name, scope.scope.as_str()) {
            (Some(name), _) => name.clone(),
            (None, "lists") => "Lists".to_owned(),
            (None, other) => other.to_owned(),
        };
        let state = match (scope.in_progress, scope.state) {
            (true, _) => "syncing",
            (false, SyncState::Ready) => "ready",
            (false, SyncState::Initial) => "initial",
        };
        let mode = match scope.mode {
            SyncMode::Delta => "delta",
            SyncMode::Enumeration => "enumeration",
            SyncMode::Unknown => "?",
        };
        let synced = scope
            .last_success_at
            .map_or_else(|| "never".to_owned(), |at| ago(now - at));
        lines.push(Line::raw(format!(
            " {:<24}{state:<9}{mode:<13}{synced:<14}{}",
            truncate(&name, 23),
            scope.last_changed_count
        )));
        if let Some(error) = &scope.last_error {
            lines.push(Line::styled(
                format!("   failed: {} ({})", error.message, error.kind),
                Style::default().fg(ERROR),
            ));
        }
    }
    lines
}

/// "Needs attention" and its problems, or that all is well.
fn problems(page: &Diagnostics) -> Vec<Line<'static>> {
    let problems = page.problems();
    let mut lines = vec![Line::default()];
    if problems.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(format!("{:<12}", "Status"), Style::default().fg(DIM)),
            Span::styled("all good", Style::default().fg(ACCENT)),
        ]));
        return lines;
    }
    lines.push(Line::styled(
        "Needs attention",
        Style::default().fg(AMBER).add_modifier(Modifier::BOLD),
    ));
    lines.extend(
        problems
            .into_iter()
            .map(|problem| Line::styled(format!(" - {problem}"), Style::default().fg(AMBER))),
    );
    lines
}

fn sync_summary(report: &DoctorReport) -> String {
    let ready = report
        .scopes
        .iter()
        .filter(|scope| scope.state == SyncState::Ready)
        .count();
    let delta = report
        .scopes
        .iter()
        .filter(|scope| scope.mode == SyncMode::Delta)
        .count();
    let mut sync = format!(
        "{ready} of {} scopes ready, {delta} on delta",
        report.scopes.len()
    );
    if report.syncing {
        sync.push_str("; syncing now");
    }
    sync
}

fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(width.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

fn human_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    match bytes {
        b if b >= KIB * KIB => format!("{:.1} MiB", b as f64 / (KIB * KIB) as f64),
        b if b >= KIB => format!("{:.1} KiB", b as f64 / KIB as f64),
        b => format!("{b} bytes"),
    }
}
