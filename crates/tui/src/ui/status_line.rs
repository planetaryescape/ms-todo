use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{App, Connection, Level};

/// The daemon connection, the last sync and the outbox depth; a banner,
/// while one shows, takes its place.
pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    if let Some(banner) = &app.banner {
        let style = match banner.level {
            Level::Info => theme.banner_info,
            Level::Error => theme.banner_error,
        };
        frame.render_widget(
            Paragraph::new(format!(" {}", banner.text)).style(style),
            area,
        );
        return;
    }
    let glyphs = &app.glyphs;
    let dim = theme.text_dim;
    let muted = theme.text_muted;
    let mut spans = Vec::new();
    if !app.selection.is_empty() {
        spans.push(Span::styled(
            format!(" {} {} selected", glyphs.selected, app.selection.len()),
            theme.key,
        ));
        spans.push(Span::styled("  · ", muted));
    }
    spans.extend(match &app.connection {
        Connection::Connected => vec![Span::styled(
            format!(" {} daemon", glyphs.connected),
            theme.accent,
        )],
        Connection::Connecting => vec![Span::styled(
            format!(" {} connecting", glyphs.disconnected),
            dim,
        )],
        Connection::Lost(why) => vec![Span::styled(
            format!(" {} daemon unreachable: {why}", glyphs.disconnected),
            theme.error,
        )],
    });
    spans.push(Span::styled("  ·  ", muted));
    let activity = &app.activity;
    spans.push(match (&activity.last_error, activity.in_progress) {
        (_, true) => Span::styled("syncing…", theme.accent),
        (Some(error), false) => {
            Span::styled(format!("sync failed: {}", error.message), theme.warning)
        }
        (None, false) => match activity.last_finished_at {
            Some(at) => Span::styled(format!("synced {}", ago(app.clock.unix() - at)), dim),
            None => Span::styled("not synced yet", dim),
        },
    });
    spans.push(Span::styled("  ·  ", muted));
    let outbox = &app.outbox;
    let waiting = outbox.pending + outbox.inflight;
    let mut parts = Vec::new();
    if waiting > 0 {
        parts.push(Span::styled(format!("{waiting} pending"), dim));
    }
    if outbox.unknown > 0 {
        parts.push(Span::styled(
            format!("{} unknown", outbox.unknown),
            theme.sync_unknown,
        ));
    }
    if outbox.failed > 0 {
        parts.push(Span::styled(
            format!("{} failed", outbox.failed),
            theme.sync_failed,
        ));
    }
    if parts.is_empty() {
        spans.push(Span::styled("all changes sent", dim));
    } else {
        spans.push(Span::styled("outbox: ", dim));
        for (index, part) in parts.into_iter().enumerate() {
            if index > 0 {
                spans.push(Span::styled(", ", dim));
            }
            spans.push(part);
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

pub(super) fn ago(seconds: i64) -> String {
    match seconds.max(0) {
        seconds if seconds < 60 => format!("{seconds}s ago"),
        seconds if seconds < 3600 => format!("{}m ago", seconds / 60),
        seconds => format!("{}h ago", seconds / 3600),
    }
}
