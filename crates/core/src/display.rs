//! Text from Graph (titles, list names, notes) shown on a terminal. It can
//! hold anything a phone or a shared list put there, including escape
//! sequences that would drive the terminal, such as OSC 52 writing to the
//! clipboard. Every place that writes such text to a terminal as-is goes
//! through here; JSON and CSV escape it themselves.

use std::borrow::Cow;

/// Shown in place of a control character.
pub const CONTROL_PLACEHOLDER: char = '\u{fffd}';

/// `text` with every control character (C0 with ESC and BEL, DEL, and C1,
/// U+0080 to U+009F) replaced by U+FFFD, except newlines, which notes need.
/// Borrowed when there's nothing to replace, which is nearly always.
pub fn display_safe(text: &str) -> Cow<'_, str> {
    if !text.chars().any(unsafe_char) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(
        text.chars()
            .map(|ch| {
                if unsafe_char(ch) {
                    CONTROL_PLACEHOLDER
                } else {
                    ch
                }
            })
            .collect(),
    )
}

/// `text` for one line, such as a window title: control characters and
/// newlines both removed.
pub fn one_line_safe(text: &str) -> String {
    text.chars().filter(|ch| !ch.is_control()).collect()
}

fn unsafe_char(ch: char) -> bool {
    // `is_control` is Unicode's Cc: exactly C0, DEL and C1.
    ch.is_control() && ch != '\n'
}

#[cfg(test)]
mod tests {
    use super::*;

    const OSC52: &str = "Pay\x1b]52;c;aGk=\x07 rent\u{9b}2J\x7f";

    #[test]
    fn escape_sequences_lose_their_control_characters() {
        let safe = display_safe(OSC52);
        assert!(
            !safe.contains(['\x1b', '\x07', '\u{9b}', '\x7f']),
            "{safe:?}"
        );
        assert_eq!(safe, "Pay\u{fffd}]52;c;aGk=\u{fffd} rent\u{fffd}2J\u{fffd}");
        assert_eq!(one_line_safe("a\nb\x1b[0m\t"), "ab[0m");
    }

    #[test]
    fn plain_text_and_newlines_are_kept_without_copying() {
        assert!(matches!(
            display_safe("Buy milk\nand eggs"),
            Cow::Borrowed(_)
        ));
        assert_eq!(display_safe("café ☕"), "café ☕");
    }
}
