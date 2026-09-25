//! My Day in the TUI (docs/blueprint/08-tui.md): `t` puts the selection,
//! or the task under the cursor, in today's My Day, or takes it out when
//! it's all there already. In the My Day view, a suggestion under the
//! cursor goes in with the same key.

use ms_todo_protocol::{Applied, TaskAction, TaskChange};

use super::{App, Effect, Level, Write, change};

impl App {
    pub(super) fn toggle_my_day(&mut self) -> Vec<Effect> {
        let targets = self.targets();
        if targets.is_empty() {
            return Vec::new();
        }
        let today = self.my_day();
        let out: Vec<String> = targets
            .iter()
            .filter(|task| task.my_day != Some(today))
            .map(|task| task.id.clone())
            .collect();
        let effect = if out.is_empty() {
            let ids = targets.iter().map(|task| task.id.clone()).collect();
            change(Write::MyDay, ids, TaskChange::RemoveFromMyDay)
        } else {
            change(Write::MyDay, out, TaskChange::AddToMyDay)
        };
        self.selection.clear();
        vec![effect]
    }

    /// Say what `t` did: the rows have moved already.
    pub(super) fn my_day_changed(&mut self, applied: &Applied) {
        let what = match applied.items.as_slice() {
            [] => return self.show(Level::Info, "Nothing to change in My Day"),
            [task] => format!(
                "\"{}\"",
                task.get("title")
                    .and_then(|title| title.as_str())
                    .unwrap_or_default()
            ),
            many => format!("{} tasks", many.len()),
        };
        let text = if applied.action == TaskAction::MyDayRemove {
            format!("Took {what} out of My Day; u puts it back")
        } else {
            format!("Added {what} to My Day; u takes it out")
        };
        self.show(Level::Info, &text);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use ms_todo_protocol::{MyDaySeed, Request, Scope, Seed, TaskAction, TaskChange};
    use serde_json::json;

    use crate::action::Action;
    use crate::app::tests::{act, answer_seed, applied, seed, seeded, task};
    use crate::app::{App, Mode, Msg, Pane, Write};

    /// My Day on Thu 24 Sep: two tasks in it, and three suggestions.
    pub(crate) fn my_day_seed() -> Seed {
        let today = json!({ "extensions": [{ "myDay": "2026-09-24" }] });
        let due = |day: &str| json!({ "dueDateTime": { "dateTime": format!("{day}T00:00:00.0000000"), "timeZone": "UTC" } });
        let mut seed = seed(
            Scope::MyDay,
            vec![
                task("m1", "Call the bank", today.clone()),
                task(
                    "m2",
                    "Pay the plumber",
                    json!({ "status": "completed", "extensions": [{ "myDay": "2026-09-24" }] }),
                ),
            ],
        );
        let suggested = |id: &str, title: &str, why: &str, mut extra: serde_json::Value| {
            extra["suggestion"] = json!(why);
            task(id, title, extra)
        };
        seed.counts.my_day = 1;
        seed.my_day = Some(MyDaySeed {
            date: "2026-09-24".into(),
            suggestions: vec![
                suggested("s1", "Renew passport", "due_today", due("2026-09-24")),
                suggested("s2", "File expenses", "overdue", due("2026-09-20")),
                suggested("s3", "Book dentist", "left_over", json!({})),
            ],
        });
        seed
    }

    /// The My Day view, seeded, with focus on its tasks.
    pub(crate) fn in_my_day() -> App {
        let mut app = seeded();
        app.focus = Pane::Sidebar;
        let effects = act(&mut app, Action::JumpTop);
        assert_eq!(
            effects[0].request,
            Request::Seed {
                scope: Some(Scope::MyDay),
                search: None
            }
        );
        answer_seed(&mut app, &effects[0], my_day_seed());
        app.focus = Pane::Tasks;
        app
    }

    fn change_of(effects: &[crate::app::Effect]) -> (Vec<String>, TaskChange) {
        match &effects[0].request {
            Request::ChangeTasks { tasks, change, .. } => (tasks.clone(), change.clone()),
            other => unreachable!("{other:?}"),
        }
    }

    #[test]
    fn my_day_is_the_first_view_with_its_day_and_suggestions_after_its_tasks() {
        let app = in_my_day();
        assert_eq!(app.sidebar_index, 0);
        assert_eq!(app.view_name(), "My Day \u{b7} Thu 24 Sep");
        let rows: Vec<(&str, bool)> = app
            .tasks
            .iter()
            .map(|task| (task.title.as_str(), task.suggestion.is_some()))
            .collect();
        assert_eq!(
            rows,
            [
                ("Call the bank", false),
                ("Pay the plumber", false),
                ("Renew passport", true),
                ("File expenses", true),
                ("Book dentist", true),
            ]
        );
    }

    #[test]
    fn t_on_a_suggestion_adds_it_and_the_row_moves_up() {
        let mut app = in_my_day();
        app.task_index = 3;
        let effects = act(&mut app, Action::ToggleMyDay);
        assert_eq!(effects[0].tag, crate::app::Tag::Write(Write::MyDay));
        assert_eq!(
            change_of(&effects),
            (vec!["s2".to_owned()], TaskChange::AddToMyDay)
        );
        let added = task(
            "s2",
            "File expenses",
            json!({ "sync_state": "pending", "extensions": [{ "myDay": "2026-09-24" }] }),
        );
        app.update(Msg::Response {
            tag: effects[0].tag,
            result: Ok(applied(TaskAction::MyDayAdd, vec![added])),
        });
        let titles: Vec<&str> = app.tasks.iter().map(|task| task.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "Call the bank",
                "File expenses",
                "Pay the plumber",
                "Renew passport",
                "Book dentist"
            ]
        );
        let banner = app.banner.as_ref().map(|banner| banner.text.as_str());
        assert_eq!(
            banner,
            Some("Added \"File expenses\" to My Day; u takes it out")
        );
    }

    #[test]
    fn t_on_a_task_in_my_day_takes_it_out() {
        let mut app = in_my_day();
        app.task_index = 0;
        let effects = act(&mut app, Action::ToggleMyDay);
        assert_eq!(
            change_of(&effects),
            (vec!["m1".to_owned()], TaskChange::RemoveFromMyDay)
        );
        let out = task("m1", "Call the bank", json!({ "sync_state": "pending" }));
        app.update(Msg::Response {
            tag: effects[0].tag,
            result: Ok(applied(TaskAction::MyDayRemove, vec![out])),
        });
        assert!(app.tasks.iter().all(|task| task.id != "m1"));
    }

    #[test]
    fn t_on_a_selection_adds_those_not_in_and_only_when_all_are_in_takes_them_out() {
        let mut app = seeded();
        // Home's tasks: none is in My Day.
        act(&mut app, Action::ToggleSelect);
        act(&mut app, Action::MoveDown);
        act(&mut app, Action::ToggleSelect);
        let effects = act(&mut app, Action::ToggleMyDay);
        let (ids, change) = change_of(&effects);
        assert_eq!(change, TaskChange::AddToMyDay);
        assert_eq!(ids.len(), 2);
        assert!(app.selection.is_empty());

        let mut app = in_my_day();
        app.task_index = 0;
        act(&mut app, Action::ToggleSelect);
        act(&mut app, Action::MoveDown);
        act(&mut app, Action::ToggleSelect);
        let (ids, change) = change_of(&act(&mut app, Action::ToggleMyDay));
        assert_eq!(change, TaskChange::RemoveFromMyDay);
        assert_eq!(ids.len(), 2);
    }

    #[test]
    fn adding_from_my_day_puts_the_task_in_it() {
        let mut app = in_my_day();
        act(&mut app, Action::Add);
        for ch in "Call mum".chars() {
            app.update(Msg::Char(ch));
        }
        assert!(matches!(app.mode, Mode::Adding { .. }));
        let effects = act(&mut app, Action::Submit);
        let added = effects
            .iter()
            .map(|effect| &effect.request)
            .find(|request| matches!(request, Request::AddTask { .. }));
        let Some(Request::AddTask { task, .. }) = added else {
            unreachable!("{effects:?}");
        };
        assert!(task.my_day);
        assert_eq!(task.list, None, "the default list, as from any view");
    }
}
