//! Last write wins, with a warning (04#conflicts, D-065). When a task
//! PATCH gets a 412 and Graph's copy changed a field the PATCH also sets,
//! the PATCH is sent again over it, and what it overwrote is written in
//! the operation's note and sent to clients as `ConflictOverwritten`.
//!
//! Two edits are still refused rather than overwritten: completing a
//! recurring task (a due date moved on another device means someone
//! completed that occurrence, and sending ours would complete the next),
//! and an edit whose flag claims the value as ms-todo's (`send`'s
//! `claims_ownership`).

use ms_todo_core::one_line_safe;
use ms_todo_store::Entity;
use serde_json::Value;

/// How much of an overwritten text value the note keeps.
const SHOWN_CHARS: usize = 40;

/// The note for an edit that overwrote `fields`, which `theirs` (Graph's
/// copy before ours landed) had changed.
pub(super) fn note(fields: &[String], theirs: &Entity) -> String {
    let fields: Vec<String> = fields
        .iter()
        .map(|field| match theirs.get(field) {
            Some(Value::String(text)) => format!("{field} (was {:?} there)", shown(text)),
            _ => field.clone(),
        })
        .collect();
    format!(
        "overwrote a change made on another device since ms-todo last read the task: {}",
        fields.join(", ")
    )
}

fn shown(text: &str) -> String {
    let text = one_line_safe(text);
    match text.char_indices().nth(SHOWN_CHARS) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_note_names_each_field_and_what_a_text_one_held() {
        let theirs = json!({
            "title": "Almond milk",
            "dueDateTime": { "dateTime": "2026-10-01T00:00:00", "timeZone": "UTC" }
        });
        let theirs = theirs.as_object().cloned().expect("object");
        let note = note(&["title".into(), "dueDateTime".into()], &theirs);
        assert!(
            note.ends_with("title (was \"Almond milk\" there), dueDateTime"),
            "{note}"
        );
    }

    #[test]
    fn a_long_or_multi_line_value_is_cut_to_one_short_line() {
        let long = format!("{}\nsecond line", "x".repeat(60));
        assert_eq!(shown(&long), format!("{}…", "x".repeat(40)));
        assert_eq!(shown("a\u{7}b"), "ab");
    }
}
