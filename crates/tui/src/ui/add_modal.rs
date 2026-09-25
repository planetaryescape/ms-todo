//! Quick add's modal (`a`), in the middle of the screen over a dimmed
//! background: the text with what was recognised highlighted by kind, the
//! task it makes, anything that wasn't used and why, and the keys. The
//! status line and hint bar stay as they are.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use super::{centered, line_input, pane};
use crate::app::App;
use crate::app::line_editor::LineEditor;
use crate::keybindings::{Context, hints};
use crate::theme::Theme;
use ms_todo_nlp::{ParsedTask, SpanKind};

/// The most warnings shown; the rest are the same kind of thing.
const MAX_WARNINGS: usize = 3;
/// The most rows the text takes before it scrolls out of the top.
const MAX_INPUT_ROWS: usize = 4;

pub fn draw(frame: &mut Frame, app: &App, input: &LineEditor, parsed: Option<&ParsedTask>) {
    let theme = &app.theme;
    let screen = frame.area();
    // Everything above the status line and the hint bar.
    let behind = Rect {
        height: screen.height.saturating_sub(2),
        ..screen
    };
    frame.buffer_mut().set_style(behind, theme.backdrop);

    // About 60% of the width; the whole width, less a margin, when narrow.
    let width = (screen.width * 3 / 5)
        .clamp(56, 100)
        .min(screen.width.saturating_sub(2));
    let inner = usize::from(width.saturating_sub(4)).max(1);

    let target = app.add_target();
    let text = input.text();
    let cursor = input.cursor().1;
    let marks: Vec<_> = parsed
        .map(|parsed| {
            parsed
                .spans
                .iter()
                .map(|span| (span.start..span.end, mark(theme, span.kind)))
                .collect()
        })
        .unwrap_or_default();
    let mut typed = vec![Span::styled("> ", theme.accent)];
    // Made safe piece by piece, so the marks' byte ranges still fit.
    typed.extend(
        line_input::highlighted(&text, cursor, Style::default(), &marks, &app.glyphs, theme)
            .into_iter()
            .map(|span| Span::styled(ms_todo_core::one_line_safe(&span.content), span.style)),
    );
    let mut input_rows = hard_wrap(typed, inner);
    // Keep the cursor's row in view: the text's end, where typing is.
    let cursor_row = (cursor + 2) / inner;
    let skip = (cursor_row + 1).saturating_sub(MAX_INPUT_ROWS);
    input_rows = input_rows
        .into_iter()
        .skip(skip)
        .take(MAX_INPUT_ROWS)
        .collect();

    let preview = match parsed {
        None => "Literal: the text is the title, as typed; Ctrl-r reads it again".to_owned(),
        Some(parsed) => {
            let mut parts = vec![target.clone()];
            parts.extend(parsed.summary(&app.parse_context()));
            format!("\u{2192} {}", parts.join(" \u{b7} "))
        }
    };
    let mut lines = input_rows;
    lines.extend(
        wrap_words(&ms_todo_core::one_line_safe(&preview), inner)
            .into_iter()
            .map(|row| Line::styled(row, theme.text_dim)),
    );
    for warning in parsed
        .map(|parsed| parsed.warnings.as_slice())
        .unwrap_or_default()
        .iter()
        .take(MAX_WARNINGS)
    {
        lines.extend(
            wrap_words(&ms_todo_core::one_line_safe(warning), inner)
                .into_iter()
                .map(|row| Line::styled(row, theme.warning)),
        );
    }
    lines.push(Line::raw(""));
    // The keys, `Enter add · Tab complete`, a whole pair to a row.
    let mut row: Vec<Span> = Vec::new();
    let mut used = 0;
    for (key, label) in hints(Context::Adding) {
        let label = label.to_lowercase();
        let width = key.chars().count() + 1 + label.chars().count();
        if used > 0 && used + 3 + width > inner {
            lines.push(Line::from(std::mem::take(&mut row)));
            used = 0;
        }
        if used > 0 {
            row.push(Span::styled(" \u{b7} ", theme.text_muted));
            used += 3;
        }
        row.push(Span::styled(key, theme.key));
        row.push(Span::styled(format!(" {label}"), theme.text_dim));
        used += width;
    }
    lines.push(Line::from(row));

    let height = u16::try_from(lines.len() + 2).unwrap_or(u16::MAX);
    let area = centered(behind, width, height);
    frame.render_widget(Clear, area);
    let title = ms_todo_core::one_line_safe(&format!(" Add task \u{2192} {target} "));
    let block = pane(theme, title, true);
    let body = block.inner(area);
    frame.render_widget(block, area);
    let padded = Rect {
        x: body.x.saturating_add(1),
        width: body.width.saturating_sub(2),
        ..body
    };
    frame.render_widget(Paragraph::new(lines), padded);
}

/// The style a recognised part is drawn in.
fn mark(theme: &Theme, kind: SpanKind) -> Style {
    match kind {
        SpanKind::Date | SpanKind::Start | SpanKind::Reminder => theme.nlp_date,
        SpanKind::List => theme.nlp_list,
        SpanKind::Label => theme.nlp_label,
        SpanKind::Priority => theme.nlp_priority,
        SpanKind::Recurrence => theme.nlp_recurrence,
        SpanKind::MyDay => theme.text_dim,
        SpanKind::Syntax => theme.text_muted,
    }
}

/// `spans` cut into rows of `width` characters, as a text field wraps.
fn hard_wrap(spans: Vec<Span<'static>>, width: usize) -> Vec<Line<'static>> {
    let mut rows = vec![Vec::new()];
    let mut used = 0;
    for span in spans {
        let style = span.style;
        let mut rest: Vec<char> = span.content.chars().collect();
        while !rest.is_empty() {
            if used == width {
                rows.push(Vec::new());
                used = 0;
            }
            let take = rest.len().min(width - used);
            let piece: String = rest.drain(..take).collect();
            used += take;
            if let Some(row) = rows.last_mut() {
                row.push(Span::styled(piece, style));
            }
        }
    }
    rows.into_iter().map(Line::from).collect()
}

/// `text` in rows of at most `width` characters, broken between words; a
/// word longer than a row is cut.
fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let mut rows: Vec<String> = Vec::new();
    let mut row = String::new();
    for word in text.split(' ') {
        let fits = row.chars().count() + 1 + word.chars().count() <= width;
        if !row.is_empty() && !fits {
            rows.push(std::mem::take(&mut row));
        }
        if !row.is_empty() {
            row.push(' ');
        }
        row.push_str(word);
        while row.chars().count() > width {
            let head: String = row.chars().take(width).collect();
            row = row.chars().skip(width).collect();
            rows.push(head);
        }
    }
    rows.push(row);
    rows
}
