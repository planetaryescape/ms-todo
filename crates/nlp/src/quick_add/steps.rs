//! Steps typed with the task (D-068): `Pack for trip :: passport; charger;
//! socks`. Everything after the first ` :: ` is the steps, split on `;`,
//! taken literally: a date or a `#` in a step is the step's text, not the
//! task's.
//!
//! ` :: ` needs a space on each side, so `std::fs` and `12::30` stay text,
//! and one inside quotes is the title's (`"a :: b"`). With no step text
//! after it (`Plan ::`), it's text too.

/// Where the steps start (the byte of the first `:`), and each step's
/// text, trimmed, with empty ones left out.
pub(super) fn split(input: &str) -> Option<(usize, Vec<String>)> {
    let marker = marker(input)?;
    let steps: Vec<String> = input[marker + 2..]
        .split(';')
        .map(str::trim)
        .filter(|step| !step.is_empty())
        .map(str::to_owned)
        .collect();
    (!steps.is_empty()).then_some((marker, steps))
}

/// The first ` :: ` outside quotes, as the quotes pass pairs them: an
/// escaped quote (`\"`) is a character, and one with no partner is too.
fn marker(input: &str) -> Option<usize> {
    let bytes = input.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'\\'
                if matches!(
                    bytes.get(at + 1),
                    Some(b'#' | b'@' | b'!' | b'^' | b'*' | b'+' | b'"' | b'\\')
                ) =>
            {
                at += 2;
            }
            // A name's quote (`#"…"`) closes at the next quote; any other
            // passes over `\"` inside it, as the quotes pass does.
            b'"' => {
                let close = if at > 0 && matches!(bytes[at - 1], b'#' | b'@') {
                    input[at + 1..].find('"').map(|to| at + 1 + to)
                } else {
                    super::passes::closing_quote(input, at)
                };
                at = close.map_or(at + 1, |close| close + 1);
            }
            b':' if bytes.get(at + 1) == Some(&b':')
                && input[..at].ends_with(char::is_whitespace)
                && input[at + 2..].starts_with(char::is_whitespace) =>
            {
                return Some(at);
            }
            _ => at += 1,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steps(input: &str) -> Option<Vec<String>> {
        split(input).map(|(_, steps)| steps)
    }

    #[test]
    fn steps_follow_the_marker_split_on_semicolons() {
        assert_eq!(
            steps("Pack for trip :: passport; charger;socks"),
            Some(vec!["passport".into(), "charger".into(), "socks".into()])
        );
        assert_eq!(split("Pack :: passport").map(|(at, _)| at), Some(5));
        // Empty steps go; a marker with none is text.
        assert_eq!(
            steps("Pack :: ; passport ;;"),
            Some(vec!["passport".into()])
        );
        assert_eq!(steps("Plan :: "), None);
        assert_eq!(steps("Plan ::"), None);
    }

    #[test]
    fn only_a_spaced_marker_outside_quotes_counts() {
        assert_eq!(steps("Read std::fs docs"), None);
        assert_eq!(steps(":: at the start"), None);
        assert_eq!(steps("Plan ::step"), None);
        assert_eq!(steps("\"Ratio a :: b\" today"), None);
        assert_eq!(
            steps("\"a :: b\" :: c"),
            Some(vec!["c".into()]),
            "the first marker outside the quotes"
        );
        assert_eq!(steps("12\" pizza :: box"), Some(vec!["box".into()]));
        assert_eq!(steps("say \\\" :: hi"), Some(vec!["hi".into()]));
        // An escaped quote inside quotes doesn't close them.
        assert_eq!(steps("\"Say \\\"hello :: world\" tomorrow"), None);
        assert_eq!(
            steps("\"Say \\\"hi\\\"\" :: wave"),
            Some(vec!["wave".into()])
        );
    }
}
