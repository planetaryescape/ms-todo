//! `o` and `y` on a task (D-050): with one link, open or copy it at once;
//! with several, pick one. Opening and copying are the runner's I/O: this
//! checks the link and says what to do.

use ms_todo_core::links::{Link, openable};

use super::{App, Level, LocalEffect, Mode};

impl App {
    /// `o` (or `y` to copy) on the task under the cursor.
    pub(super) fn follow_link(&mut self, copy: bool) {
        let Some(task) = self.selected() else {
            return;
        };
        let mut links = task.links();
        match links.len() {
            0 => self.show(Level::Info, "This task has no links"),
            1 => {
                let link = links.remove(0);
                self.use_link(&link, copy);
            }
            _ => self.mode = Mode::Links { links, index: 0 },
        }
    }

    /// Enter, `o` or `y` in the link picker.
    pub(super) fn pick_link(&mut self, copy: bool) {
        let Mode::Links { links, index } = std::mem::replace(&mut self.mode, Mode::Normal) else {
            return;
        };
        if let Some(link) = links.get(index) {
            self.use_link(link, copy);
        }
    }

    fn use_link(&mut self, link: &Link, copy: bool) {
        if copy {
            self.local = Some(LocalEffect::Copy(link.url.clone()));
            self.show(Level::Info, &format!("Copied {}", link.url));
            return;
        }
        match openable(&link.url) {
            Ok(url) => {
                self.show(Level::Info, &format!("Opening {url}"));
                self.local = Some(LocalEffect::Open(url));
            }
            Err(why) => self.show(
                Level::Error,
                &format!("Not opening {}: {why}; y copies it", link.text),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::action::Action;
    use crate::app::Task;
    use crate::app::tests::{act, seeded, task};

    fn with_notes(notes: &str, linked: serde_json::Value) -> App {
        let mut app = seeded();
        let entity = task(
            "t9",
            "Read up",
            json!({ "body": { "content": notes, "contentType": "text" }, "linkedResources": linked }),
        );
        app.tasks[0] = Task::from_entity(&entity).expect("task");
        app.task_index = 0;
        app
    }

    #[test]
    fn one_link_opens_at_once() {
        let mut app = with_notes("see https://example.com/a.", json!([]));
        assert!(
            act(&mut app, Action::OpenLink).is_empty(),
            "nothing for the daemon"
        );
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(
            app.local,
            Some(LocalEffect::Open(
                url::Url::parse("https://example.com/a").expect("url")
            ))
        );
    }

    #[test]
    fn y_copies_and_says_so() {
        let mut app = with_notes("https://example.com/a", json!([]));
        act(&mut app, Action::CopyLink);
        assert_eq!(
            app.local,
            Some(LocalEffect::Copy("https://example.com/a".into()))
        );
        let banner = app.banner.as_ref().expect("banner");
        assert!(banner.text.starts_with("Copied"), "{}", banner.text);
    }

    #[test]
    fn several_links_open_a_picker_linked_resources_first() {
        let mut app = with_notes(
            "https://notes.example.com and [docs](https://docs.example.com)",
            json!([{ "webUrl": "https://mail.example.com/1", "displayName": "The email" }]),
        );
        act(&mut app, Action::OpenLink);
        let texts: Vec<String> = match &app.mode {
            Mode::Links { links, index: 0 } => links.iter().map(|link| link.text.clone()).collect(),
            other => vec![format!("no picker: {other:?}")],
        };
        assert_eq!(texts, ["The email", "notes.example.com", "docs"]);
        assert_eq!(app.context(), crate::keybindings::Context::Links);
        act(&mut app, Action::MoveDown);
        act(&mut app, Action::MoveDown);
        act(&mut app, Action::MoveDown);
        act(&mut app, Action::Submit);
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(
            app.local,
            Some(LocalEffect::Open(
                url::Url::parse("https://docs.example.com").expect("url")
            ))
        );
    }

    #[test]
    fn y_in_the_picker_copies_and_escape_does_nothing() {
        let mut app = with_notes("https://a.example.com https://b.example.com", json!([]));
        act(&mut app, Action::CopyLink);
        act(&mut app, Action::MoveDown);
        act(&mut app, Action::CopyLink);
        assert_eq!(
            app.local,
            Some(LocalEffect::Copy("https://b.example.com".into()))
        );
        app.local = None;
        act(&mut app, Action::OpenLink);
        act(&mut app, Action::Cancel);
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.local, None);
    }

    #[test]
    fn other_schemes_are_listed_but_not_opened() {
        let mut app = with_notes(
            "",
            json!([{ "webUrl": "javascript:alert(1)", "displayName": "Evil" }]),
        );
        act(&mut app, Action::OpenLink);
        assert_eq!(app.local, None);
        let banner = app.banner.as_ref().expect("banner");
        assert_eq!(banner.level, Level::Error);
        assert!(
            banner.text.contains("only http, https and mailto"),
            "{}",
            banner.text
        );
    }

    #[test]
    fn no_links_says_so() {
        let mut app = with_notes("plain notes", json!([]));
        act(&mut app, Action::OpenLink);
        assert_eq!(app.local, None);
        assert_eq!(
            app.banner.as_ref().map(|banner| banner.text.as_str()),
            Some("This task has no links")
        );
    }
}
