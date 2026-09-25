use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

use crate::app::line_editor::LineEditor;
use crate::glyphs::Glyphs;

/// A one-line editor's text with its cursor.
pub fn single(input: &LineEditor, style: Style, glyphs: &Glyphs) -> Vec<Span<'static>> {
    spans(&input.text(), Some(input.cursor().1), style, glyphs)
}

/// One line of a line editor's text, with the cursor when it's on this
/// line: the character under it reversed, or past the end, the cursor
/// glyph.
pub fn spans(
    text: &str,
    cursor: Option<usize>,
    style: Style,
    glyphs: &Glyphs,
) -> Vec<Span<'static>> {
    let Some(at) = cursor else {
        return vec![Span::styled(text.to_owned(), style)];
    };
    let before: String = text.chars().take(at).collect();
    let mut rest = text.chars().skip(at);
    let mut spans = vec![Span::styled(before, style)];
    match rest.next() {
        Some(under) => {
            spans.push(Span::styled(
                under.to_string(),
                style.add_modifier(Modifier::REVERSED),
            ));
            spans.push(Span::styled(rest.collect::<String>(), style));
        }
        None => spans.push(Span::styled(glyphs.cursor, style)),
    }
    spans
}
