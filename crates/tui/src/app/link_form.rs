//! Editing a task's link in the detail pane (D-066): its URL, and with
//! Tab its name, app and external ID, the fields `tasks link add|edit`
//! take. Enter sends every field that changed in one change. Graph can't
//! clear a link's field (S15), so emptying one that has a value is
//! refused here rather than sent.

use ms_todo_core::links::{openable, storable};
use ms_todo_protocol::{LinkEdit, NewLink, TaskChange};

use super::{App, Level, LineEditor, Mode, Task};

/// One field of a link, in the order Tab visits them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LinkPart {
    #[default]
    Url,
    Name,
    App,
    ExternalId,
}

impl LinkPart {
    /// Its label in the detail pane and the hint bar.
    pub fn label(self) -> &'static str {
        match self {
            Self::Url => "Link URL",
            Self::Name => "Link name",
            Self::App => "Link app",
            Self::ExternalId => "Link ID",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Url => Self::Name,
            Self::Name => Self::App,
            Self::App => Self::ExternalId,
            Self::ExternalId => Self::Url,
        }
    }

    pub const ALL: [Self; 4] = [Self::Url, Self::Name, Self::App, Self::ExternalId];
}

/// The link being edited: which one, the field being typed, and what the
/// others hold so far.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LinkForm {
    /// The link's ID; `None` adds one.
    pub id: Option<String>,
    pub part: LinkPart,
    /// Each field's text, by [`LinkPart::ALL`]; the one being typed is in
    /// the prompt's input until Tab or Enter.
    pub values: [String; 4],
}

impl LinkForm {
    /// The task's link as it is, or an empty one to add.
    pub fn of(task: &Task) -> Self {
        Self {
            id: task.link_id.clone(),
            part: LinkPart::Url,
            values: current(task),
        }
    }

    pub fn value(&self, part: LinkPart) -> &str {
        &self.values[part as usize]
    }
}

/// The task's link's fields, by [`LinkPart::ALL`]; empty where it has
/// none.
fn current(task: &Task) -> [String; 4] {
    let (url, name) = task
        .linked
        .first()
        .map(|(url, name)| (url.clone(), name.clone().unwrap_or_default()))
        .unwrap_or_default();
    [
        url,
        name,
        task.link_app.clone().unwrap_or_default(),
        task.link_external_id.clone().unwrap_or_default(),
    ]
}

impl App {
    /// Tab in the link prompt: keep what's typed and type the next field.
    pub(super) fn next_link_part(&mut self) {
        if let Mode::EditingChild {
            target: super::steps::ChildTarget::Link(form),
            input,
            error,
            ..
        } = &mut self.mode
        {
            form.values[form.part as usize] = input.text();
            form.part = form.part.next();
            *input = LineEditor::single(form.value(form.part));
            *error = None;
        }
    }

    /// Enter in the link prompt: the change the form makes to `task`'s
    /// link, `Ok(None)` when it makes none, or why it can't be sent.
    pub(super) fn link_change(
        &mut self,
        task: &Task,
        mut form: LinkForm,
        typed: String,
    ) -> Result<Option<TaskChange>, String> {
        form.values[form.part as usize] = typed;
        let [url, name, app, external_id] = form.values.map(|value| value.trim().to_owned());
        if url.is_empty() {
            return Err("Type a URL; d deletes the link".into());
        }
        if let Err(why) = storable(&url) {
            return Err(format!("Not a link: {why}"));
        }
        let Some(link) = form.id else {
            self.warn_unopenable(&url);
            let given = |value: String| (!value.is_empty()).then_some(value);
            return Ok(Some(TaskChange::AddLink(NewLink {
                url,
                name: given(name),
                app: given(app),
                external_id: given(external_id),
            })));
        };
        let before = current(task);
        let mut changed = [url, name, app, external_id]
            .into_iter()
            .zip(before)
            .zip(LinkPart::ALL)
            .map(|((now, before), part)| {
                if now == before {
                    Ok(None)
                } else if now.is_empty() {
                    Err(format!(
                        "Microsoft To Do can't clear a link's {}; type a new one, or d deletes the link",
                        part.label().trim_start_matches("Link ")
                    ))
                } else {
                    Ok(Some(now))
                }
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter();
        let mut next = || changed.next().flatten();
        let edit = LinkEdit {
            link: Some(link),
            url: next(),
            name: next(),
            app: next(),
            external_id: next(),
        };
        if edit.url.is_none()
            && edit.name.is_none()
            && edit.app.is_none()
            && edit.external_id.is_none()
        {
            return Ok(None);
        }
        if let Some(url) = &edit.url {
            self.warn_unopenable(url);
        }
        Ok(Some(TaskChange::EditLink(edit)))
    }

    fn warn_unopenable(&mut self, url: &str) {
        if openable(url).is_err() {
            self.show(
                Level::Info,
                "Kept, but only http, https and mailto links open",
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use ms_todo_protocol::Request;
    use serde_json::json;

    use super::*;
    use crate::action::Action;
    use crate::app::steps::{ChildTarget, DetailRow};
    use crate::app::tests::{act, answer_seed, scope_home, seed, seeded, task};
    use crate::app::{Effect, Msg, Pane};
    use crate::keybindings::Context;

    /// "Paint", with a named link from an app, the cursor on the link.
    fn linked() -> App {
        let mut app = seeded();
        let effects = app.update(Msg::Event(ms_todo_protocol::Event::ResyncNeeded));
        let paint = task(
            "t9",
            "Paint",
            json!({ "linkedResources": [{
                "id": "r1", "webUrl": "https://example.com/colours",
                "displayName": "Colours", "applicationName": "Figma", "externalId": "f-1"
            }] }),
        );
        answer_seed(&mut app, &effects[0], seed(scope_home(), vec![paint]));
        app.focus = Pane::Detail;
        app.detail_row = DetailRow::Link;
        app
    }

    fn typed(app: &mut App, text: &str) {
        for ch in text.chars() {
            app.update(Msg::Char(ch));
        }
    }

    fn erase(app: &mut App) {
        app.update(Msg::Key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('u'),
            crossterm::event::KeyModifiers::CONTROL,
        )));
    }

    fn change(effects: &[Effect]) -> TaskChange {
        match effects {
            [
                Effect {
                    request: Request::ChangeTasks { change, .. },
                    ..
                },
            ] => change.clone(),
            other => unreachable!("not one change: {other:?}"),
        }
    }

    #[test]
    fn tab_moves_through_the_fields_and_enter_sends_what_changed() {
        let mut app = linked();
        act(&mut app, Action::EditHere);
        assert_eq!(app.context(), Context::LinkForm);
        act(&mut app, Action::Complete);
        assert!(matches!(
            &app.mode,
            Mode::EditingChild { target: ChildTarget::Link(form), input, .. }
                if form.part == LinkPart::Name && input.text() == "Colours"
        ));
        erase(&mut app);
        typed(&mut app, "Palette");
        act(&mut app, Action::Complete);
        act(&mut app, Action::Complete);
        erase(&mut app);
        typed(&mut app, "f-2");
        assert_eq!(
            change(&act(&mut app, Action::Submit)),
            TaskChange::EditLink(LinkEdit {
                link: Some("r1".into()),
                name: Some("Palette".into()),
                external_id: Some("f-2".into()),
                ..LinkEdit::default()
            })
        );
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn a_field_graph_cant_clear_is_refused_and_nothing_changed_sends_nothing() {
        let mut app = linked();
        act(&mut app, Action::EditHere);
        act(&mut app, Action::Complete);
        act(&mut app, Action::Complete);
        erase(&mut app);
        assert!(act(&mut app, Action::Submit).is_empty());
        assert!(matches!(
            &app.mode,
            Mode::EditingChild { error: Some(why), .. } if why.contains("can't clear a link's app")
        ));
        act(&mut app, Action::Cancel);
        act(&mut app, Action::EditHere);
        act(&mut app, Action::Complete);
        assert!(act(&mut app, Action::Submit).is_empty());
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn a_new_link_takes_its_name_app_and_id_too() {
        let mut app = seeded();
        app.focus = Pane::Detail;
        app.detail_row = DetailRow::Link;
        act(&mut app, Action::EditHere);
        typed(&mut app, "https://a.example");
        act(&mut app, Action::Complete);
        typed(&mut app, "A page");
        act(&mut app, Action::Complete);
        act(&mut app, Action::Complete);
        typed(&mut app, "42");
        assert_eq!(
            change(&act(&mut app, Action::Submit)),
            TaskChange::AddLink(NewLink {
                url: "https://a.example".into(),
                name: Some("A page".into()),
                app: None,
                external_id: Some("42".into()),
            })
        );
    }
}
