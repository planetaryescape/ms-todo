//! The text typed in a prompt, with a cursor. ratatui-textarea does the
//! editing (moves, word jumps, deletes, multi-line notes); this adapter
//! only picks its keys. It's never drawn as a widget: the panes draw the
//! lines and the cursor themselves, as the rest of the TUI draws text.
//!
//! The keys, as `keybindings::EDITOR_KEYS` describes them: Left and
//! Right, Home and End (Ctrl-a, Ctrl-e), a word back and forward (Alt-b,
//! Alt-f, Ctrl- or Alt-Left and Right), Backspace and Delete, Ctrl-w a
//! word back, Ctrl-u to the line's start, Ctrl-k to its end.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui_textarea::{CursorMove, TextArea};

#[derive(Clone, Debug)]
pub struct LineEditor {
    area: TextArea<'static>,
    multiline: bool,
}

impl LineEditor {
    /// One line, with the cursor after `text`.
    pub fn single(text: &str) -> Self {
        // A single-line prompt can't hold a newline; notes use `multi`.
        Self::new(&text.replace(['\r', '\n'], " "), false)
    }

    /// Several lines, with the cursor after the last.
    pub fn multi(text: &str) -> Self {
        Self::new(text, true)
    }

    fn new(text: &str, multiline: bool) -> Self {
        let mut area = TextArea::new(text.split('\n').map(str::to_owned).collect());
        area.move_cursor(CursorMove::Bottom);
        area.move_cursor(CursorMove::End);
        Self { area, multiline }
    }

    pub fn text(&self) -> String {
        self.area.lines().join("\n")
    }

    pub fn lines(&self) -> &[String] {
        self.area.lines()
    }

    /// `(row, column)`, the column in characters.
    pub fn cursor(&self) -> (usize, usize) {
        let cursor = self.area.cursor();
        (cursor.0, cursor.1)
    }

    pub fn insert(&mut self, ch: char) {
        self.area.insert_char(ch);
    }

    /// True when a character went.
    pub fn backspace(&mut self) -> bool {
        self.area.delete_char()
    }

    /// Enter in notes. Ignored on one line.
    pub fn newline(&mut self) -> bool {
        if self.multiline {
            self.area.insert_newline();
        }
        self.multiline
    }

    /// An editing key. True when the text changed, not just the cursor.
    pub fn key(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            // textarea's Ctrl-u is undo; readline's, which people expect
            // in a prompt, deletes to the start of the line.
            KeyCode::Char('u') if ctrl => self.area.delete_line_by_head(),
            // Many macOS terminals send Option-arrow as Alt-arrow.
            KeyCode::Left if alt && !ctrl => self.moved(CursorMove::WordBack),
            KeyCode::Right if alt && !ctrl => self.moved(CursorMove::WordForward),
            KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown
                if !self.multiline =>
            {
                false
            }
            // A tab or a newline typed into a title would reach Graph as
            // is; Enter and Ctrl-s are the registry's to handle.
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Enter => false,
            KeyCode::Char('m' | 'j') if ctrl => false,
            _ => {
                // Shift would start a selection nothing draws.
                let key = KeyEvent {
                    modifiers: key.modifiers - KeyModifiers::SHIFT,
                    ..key
                };
                self.area.input(key)
            }
        }
    }

    fn moved(&mut self, to: CursorMove) -> bool {
        self.area.move_cursor(to);
        false
    }
}

/// The same text with the cursor in the same place: what the app's state
/// is made of. Undo history and the like aren't compared.
impl PartialEq for LineEditor {
    fn eq(&self, other: &Self) -> bool {
        self.multiline == other.multiline
            && self.lines() == other.lines()
            && self.cursor() == other.cursor()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(editor: &mut LineEditor, code: KeyCode, modifiers: KeyModifiers) -> bool {
        editor.key(KeyEvent::new(code, modifiers))
    }

    fn at(editor: &LineEditor) -> usize {
        editor.cursor().1
    }

    #[test]
    fn the_cursor_starts_after_the_text() {
        let editor = LineEditor::single("Pay rent");
        assert_eq!(editor.cursor(), (0, 8));
        let notes = LineEditor::multi("one\ntwo");
        assert_eq!(notes.cursor(), (1, 3));
        assert_eq!(LineEditor::single("a\nb").text(), "a b");
    }

    #[test]
    fn keys_move_the_cursor_and_typing_goes_where_it_is() {
        let none = KeyModifiers::NONE;
        let ctrl = KeyModifiers::CONTROL;
        let alt = KeyModifiers::ALT;
        let mut editor = LineEditor::single("pay the rent");
        assert!(!press(&mut editor, KeyCode::Left, none));
        assert_eq!(at(&editor), 11);
        press(&mut editor, KeyCode::Home, none);
        assert_eq!(at(&editor), 0);
        press(&mut editor, KeyCode::End, none);
        assert_eq!(at(&editor), 12);
        press(&mut editor, KeyCode::Char('a'), ctrl);
        assert_eq!(at(&editor), 0);
        press(&mut editor, KeyCode::Char('e'), ctrl);
        assert_eq!(at(&editor), 12);
        press(&mut editor, KeyCode::Char('b'), alt);
        assert_eq!(at(&editor), 8, "the start of `rent`");
        press(&mut editor, KeyCode::Left, ctrl);
        assert_eq!(at(&editor), 4, "the start of `the`");
        press(&mut editor, KeyCode::Char('f'), alt);
        press(&mut editor, KeyCode::Left, alt);
        assert_eq!(at(&editor), 4);
        press(&mut editor, KeyCode::Right, ctrl);
        assert!(at(&editor) > 4);
        press(&mut editor, KeyCode::Char('a'), ctrl);
        press(&mut editor, KeyCode::Right, none);
        editor.insert('!');
        assert_eq!(editor.text(), "p!ay the rent");
        assert_eq!(at(&editor), 2);
        // Shift doesn't start a selection that would then be typed over.
        press(&mut editor, KeyCode::Left, KeyModifiers::SHIFT);
        editor.insert('?');
        assert_eq!(editor.text(), "p?!ay the rent");
    }

    #[test]
    fn deleting_words_and_to_either_end_of_the_line() {
        let none = KeyModifiers::NONE;
        let ctrl = KeyModifiers::CONTROL;
        let mut editor = LineEditor::single("pay the rent");
        assert!(press(&mut editor, KeyCode::Char('w'), ctrl));
        assert_eq!(editor.text(), "pay the ");
        press(&mut editor, KeyCode::Left, none);
        press(&mut editor, KeyCode::Left, none);
        assert!(press(&mut editor, KeyCode::Char('k'), ctrl));
        assert_eq!(editor.text(), "pay th");
        press(&mut editor, KeyCode::Left, none);
        assert!(press(&mut editor, KeyCode::Char('u'), ctrl));
        assert_eq!(editor.text(), "h");
        assert_eq!(at(&editor), 0);
        assert!(press(&mut editor, KeyCode::Delete, none));
        assert_eq!(editor.text(), "");
        assert!(!editor.backspace(), "nothing left to erase");
    }

    #[test]
    fn one_line_takes_no_tab_or_newline_but_notes_take_newlines() {
        let mut editor = LineEditor::single("a");
        assert!(!press(&mut editor, KeyCode::Tab, KeyModifiers::NONE));
        assert!(!press(&mut editor, KeyCode::Enter, KeyModifiers::NONE));
        assert!(!press(
            &mut editor,
            KeyCode::Char('j'),
            KeyModifiers::CONTROL
        ));
        assert!(!editor.newline());
        assert_eq!(editor.text(), "a");

        let mut notes = LineEditor::multi("first");
        assert!(notes.newline());
        notes.insert('2');
        assert_eq!(notes.text(), "first\n2");
        press(&mut notes, KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(notes.cursor().0, 0);
    }

    #[test]
    fn editors_compare_by_text_and_cursor() {
        let mut moved = LineEditor::single("ab");
        press(&mut moved, KeyCode::Left, KeyModifiers::NONE);
        assert_ne!(moved, LineEditor::single("ab"));
        press(&mut moved, KeyCode::Right, KeyModifiers::NONE);
        assert_eq!(moved, LineEditor::single("ab"));
    }
}
