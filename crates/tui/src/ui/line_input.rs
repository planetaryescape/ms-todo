use ratatui::style::Style;
use ratatui::text::Span;

use crate::app::line_editor::LineEditor;
use crate::glyphs::Glyphs;
use crate::theme::Theme;

/// A one-line editor's text with its cursor.
pub fn single(
    input: &LineEditor,
    style: Style,
    glyphs: &Glyphs,
    theme: &Theme,
) -> Vec<Span<'static>> {
    spans(&input.text(), Some(input.cursor().1), style, glyphs, theme)
}

/// One line of a line editor's text, with the cursor when it's on this
/// line: the character under it in the theme's cursor, or past the end,
/// the cursor glyph.
pub fn spans(
    text: &str,
    cursor: Option<usize>,
    style: Style,
    glyphs: &Glyphs,
    theme: &Theme,
) -> Vec<Span<'static>> {
    let Some(at) = cursor else {
        return vec![Span::styled(text.to_owned(), style)];
    };
    let before: String = text.chars().take(at).collect();
    let mut rest = text.chars().skip(at);
    let mut spans = vec![Span::styled(before, style)];
    match rest.next() {
        Some(under) => {
            spans.push(Span::styled(under.to_string(), style.patch(theme.cursor)));
            spans.push(Span::styled(rest.collect::<String>(), style));
        }
        None => spans.push(Span::styled(glyphs.cursor, style.patch(theme.cursor_glyph))),
    }
    spans
}

/// One line of text with its cursor, each byte range in `marks` drawn in
/// its style over `style`: quick add's highlighting.
pub fn highlighted(
    text: &str,
    cursor: usize,
    style: Style,
    marks: &[(std::ops::Range<usize>, Style)],
    glyphs: &Glyphs,
    theme: &Theme,
) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut run = String::new();
    let mut run_style = style;
    for (index, (at, ch)) in text.char_indices().enumerate() {
        let mut cell = marks
            .iter()
            .find(|(range, _)| range.contains(&at))
            .map_or(style, |(_, mark)| style.patch(*mark));
        if index == cursor {
            cell = cell.patch(theme.cursor);
        }
        if cell != run_style && !run.is_empty() {
            spans.push(Span::styled(std::mem::take(&mut run), run_style));
        }
        run_style = cell;
        run.push(ch);
    }
    if !run.is_empty() {
        spans.push(Span::styled(run, run_style));
    }
    if cursor >= text.chars().count() {
        spans.push(Span::styled(glyphs.cursor, style.patch(theme.cursor_glyph)));
    }
    spans
}
