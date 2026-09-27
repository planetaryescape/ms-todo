//! Switching context (rung 9d): `c` steps through the contexts
//! config.toml defines, then none, and the palette picks one. The daemon
//! holds the active context, so the CLI follows the switch too; the seed
//! after it has only the context's lists and their tasks.

use ms_todo_protocol::{ErrorPayload, Request, ResponseData, Scope};

use super::{App, Effect, Level, Tag};

impl App {
    /// `c`: the next context in config.toml's order; after the last, none.
    pub(super) fn next_context(&mut self) -> Vec<Effect> {
        if self.context_names.is_empty() {
            self.show(
                Level::Info,
                "No contexts: add a [contexts.<name>] section to config.toml",
            );
            return Vec::new();
        }
        let current = self.active_context.as_ref().and_then(|active| {
            self.context_names
                .iter()
                .position(|name| *name == active.name)
        });
        let next = match current {
            None => self.context_names.first().cloned(),
            Some(at) => self.context_names.get(at + 1).cloned(),
        };
        vec![switch_context(next)]
    }

    /// The daemon's answer to a switch: read the view again in the new
    /// context. The seed that answers does the rest ([`App::left_context`]),
    /// as it does for a switch made by the CLI.
    pub(super) fn context_switched(
        &mut self,
        result: Result<ResponseData, ErrorPayload>,
    ) -> Vec<Effect> {
        match result {
            Ok(ResponseData::Contexts(contexts)) => {
                let said = match &contexts.active {
                    Some(name) => format!("Context: {name}; c for the next"),
                    None => "No context: every list".to_owned(),
                };
                self.show(Level::Info, &said);
                vec![self.seed_now()]
            }
            Ok(_) => {
                self.show(Level::Error, "The daemon sent an unexpected answer");
                Vec::new()
            }
            Err(error) => {
                self.show(Level::Error, &error.message);
                Vec::new()
            }
        }
    }
}

impl App {
    /// A seed says the context changed, whoever changed it: a list on
    /// screen that the new sidebar lacks gives way to the context's home
    /// (its default list, else its first; the empty All view with none),
    /// read once this seed lands, so nothing outside the context stays on
    /// screen or takes quick add. A context with no lists says so.
    pub(super) fn left_context(&mut self) {
        if let Some(Scope::List { id }) = &self.wanted
            && !self.lists.iter().any(|list| list.id == *id)
        {
            self.wanted = None;
            self.seeds.again = true;
        }
        if let Some(context) = &self.active_context
            && context.lists == 0
        {
            let said = format!("Context {} has no lists; `mst ctx` says why", context.name);
            self.show(Level::Info, &said);
        }
    }
}

/// Ask the daemon to make `name` the active context (`None` for none).
pub(super) fn switch_context(name: Option<String>) -> Effect {
    Effect {
        tag: Tag::Context,
        request: Request::SetContext { name },
    }
}

#[cfg(test)]
mod tests {
    use ms_todo_protocol::{AppliedContext, Contexts};

    use super::*;
    use crate::action::Action;
    use crate::app::palette::Command;
    use crate::app::tests::{act, answer_seed, home_tasks, seed, seeded};
    use crate::app::{Mode, Msg};

    fn work() -> AppliedContext {
        AppliedContext {
            name: "work".into(),
            lists: 1,
            default_list: Some("Home".into()),
        }
    }

    /// `app` after a seed that says `context` is active, of home and work.
    fn in_context(context: Option<AppliedContext>) -> App {
        let mut app = seeded();
        let effects = app.update(Msg::Event(ms_todo_protocol::Event::ResyncNeeded));
        let mut seed = seed(Scope::List { id: "home".into() }, home_tasks());
        seed.context = context;
        seed.contexts = vec!["home".into(), "work".into()];
        answer_seed(&mut app, &effects[0], seed);
        app
    }

    fn asked(effects: &[Effect]) -> Option<Option<String>> {
        match &effects.first()?.request {
            Request::SetContext { name } => Some(name.clone()),
            _ => None,
        }
    }

    #[test]
    fn c_steps_through_the_contexts_then_none() {
        let mut app = in_context(None);
        assert_eq!(
            asked(&act(&mut app, Action::NextContext)),
            Some(Some("home".into()))
        );
        let mut app = in_context(Some(work()));
        // Work is the last: after it, none.
        assert_eq!(asked(&act(&mut app, Action::NextContext)), Some(None));

        let mut bare = seeded();
        assert!(act(&mut bare, Action::NextContext).is_empty());
        assert!(
            bare.banner
                .as_ref()
                .is_some_and(|banner| banner.text.contains("[contexts."))
        );
    }

    #[test]
    fn a_switch_reads_the_view_again_in_the_new_context() {
        let mut app = in_context(None);
        let effects = app.update(Msg::Response {
            tag: Tag::Context,
            result: Ok(ResponseData::Contexts(Contexts {
                active: Some("work".into()),
                ..Contexts::default()
            })),
        });
        assert!(matches!(effects[0].request, Request::Seed { .. }));
        assert!(
            app.banner
                .as_ref()
                .is_some_and(|banner| banner.text.starts_with("Context: work"))
        );
        // The add modal names where a task from a view goes.
        let mut seed = seed(Scope::All, home_tasks());
        seed.context = Some(work());
        answer_seed(&mut app, &effects[0], seed);
        assert!(
            !app.cache
                .keys()
                .any(|scope| matches!(scope, Scope::List { .. })),
            "the old context's rows are dropped"
        );
        assert_eq!(app.add_target(), "Home");
        assert_eq!(app.active_context, Some(work()));
    }

    #[test]
    fn a_switch_made_elsewhere_leaves_a_list_the_context_lacks() {
        let mut app = seeded();
        assert_eq!(app.wanted, Some(Scope::List { id: "home".into() }));
        // The CLI switched: the daemon sends ResyncNeeded, and the seed
        // for the old list comes back in a context without it.
        let effects = app.update(Msg::Event(ms_todo_protocol::Event::ResyncNeeded));
        let mut answer = seed(Scope::List { id: "home".into() }, home_tasks());
        answer.lists.retain(|list| list["id"] != "home");
        answer.context = Some(work());
        let effects = answer_seed(&mut app, &effects[0], answer);
        assert_eq!(app.wanted, None);
        assert!(matches!(
            effects.first().map(|effect| &effect.request),
            Some(Request::Seed { scope: None, .. })
        ));

        // A list the new context keeps stays on screen.
        let mut app = in_context(None);
        let effects = app.update(Msg::Event(ms_todo_protocol::Event::ResyncNeeded));
        let mut answer = seed(Scope::List { id: "home".into() }, home_tasks());
        answer.context = Some(work());
        assert!(answer_seed(&mut app, &effects[0], answer).is_empty());
        assert_eq!(app.wanted, Some(Scope::List { id: "home".into() }));
    }

    #[test]
    fn the_palette_offers_every_other_context_and_none() {
        let mut app = in_context(Some(work()));
        let items: Vec<(String, Command)> = app
            .palette_items("context")
            .into_iter()
            .map(|item| (item.label, item.command))
            .collect();
        assert!(items.contains(&(
            "Context: home".into(),
            Command::Context(Some("home".into()))
        )));
        assert!(items.contains(&("Context: none".into(), Command::Context(None))));
        assert!(!items.iter().any(|(label, _)| label == "Context: work"));
        act(&mut app, Action::Palette);
        for ch in "context: none".chars() {
            app.update(Msg::Char(ch));
        }
        let effects = act(&mut app, Action::Submit);
        assert_eq!(asked(&effects), Some(None));
        assert_eq!(app.mode, Mode::Normal);
    }
}
