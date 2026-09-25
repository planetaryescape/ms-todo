//! `App::update` for every action and daemon message: what the state
//! becomes and which requests come out. No daemon, no terminal.

use ms_todo_protocol::{
    Candidate, Counts, EntityChanged, ErrorPayload, Event, OpError, OutboxDepth, Request,
    ResponseData, Scope, Seed, SyncActivity, SyncInfo, SyncState, TaskAction, TaskChange,
    WriteRejected,
};
use serde_json::{Value, json};

use super::*;
use crate::glyphs::UNICODE;

pub(crate) fn clock() -> Clock {
    Clock {
        // Thursday 24 September 2026, 12:00 UTC: 13:00 in London.
        now: chrono::DateTime::parse_from_rfc3339("2026-09-24T13:00:00+01:00").expect("now"),
    }
}

pub(crate) fn entity(value: Value) -> ms_todo_protocol::Entity {
    value.as_object().cloned().expect("object")
}

pub(crate) fn task(id: &str, title: &str, extra: Value) -> ms_todo_protocol::Entity {
    let mut task = entity(json!({
        "id": id, "list_id": "home", "title": title, "status": "notStarted",
        "importance": "normal", "sync_state": "synced"
    }));
    task.extend(entity(extra));
    task
}

pub(crate) fn seed(scope: Scope, tasks: Vec<ms_todo_protocol::Entity>) -> Seed {
    let ready = SyncInfo {
        state: SyncState::Ready,
        generation: 3,
    };
    Seed {
        scope: Some(scope),
        lists: vec![
            entity(
                json!({ "id": "tasks", "displayName": "Tasks", "wellknownListName": "defaultList" }),
            ),
            entity(json!({ "id": "home", "displayName": "Home" })),
        ],
        lists_sync: ready,
        counts: Counts {
            my_day: 0,
            important: 1,
            planned: 2,
            all: 3,
            completed: 1,
            assigned: 0,
            lists: [("home".to_owned(), 3)].into_iter().collect(),
        },
        tasks,
        sync: ready,
        activity: SyncActivity {
            generation: 5,
            in_progress: false,
            last_finished_at: Some(clock().unix() - 12),
            last_error: None,
        },
        outbox: OutboxDepth::default(),
        my_day: None,
    }
}

pub(crate) fn home_tasks() -> Vec<ms_todo_protocol::Entity> {
    vec![
        task(
            "t1",
            "Pay rent",
            json!({ "importance": "high",
                    "dueDateTime": { "dateTime": "2026-10-01T00:00:00.0000000", "timeZone": "Europe/London" },
                    "recurrence": { "pattern": { "type": "absoluteMonthly", "interval": 1, "dayOfMonth": 1 } } }),
        ),
        task("t2", "Call Sam", json!({ "sync_state": "pending" })),
        task(
            "t3",
            "Ship blueprint",
            json!({ "status": "completed",
                    "completedDateTime": { "dateTime": "2026-09-23T10:00:00.0000000", "timeZone": "UTC" } }),
        ),
        task("t4", "Water plants", json!({ "sync_state": "failed" })),
    ]
}

/// Connected, with Home's tasks drawn, focus on the task list.
pub(crate) fn seeded() -> App {
    let mut app = App::new(UNICODE, clock());
    // Fixed, so the snapshots don't change with each release.
    app.version = "9.9.9";
    let effects = app.update(Msg::Connected);
    assert_eq!(effects.len(), 1);
    answer_seed(&mut app, &effects[0], seed(scope_home(), home_tasks()));
    app
}

#[test]
fn signed_out_without_a_cache_shows_login_and_blocks_task_actions() {
    let mut app = App::new(UNICODE, clock())
        .with_sign_in_command(Some("ms-todo --instance scratch auth login".into()));
    let effects = app.update(Msg::Connected);
    assert!(matches!(effects[0].request, Request::Seed { .. }));
    app.update(Msg::Response {
        tag: effects[0].tag,
        result: Err(ErrorPayload {
            kind: "auth_required".into(),
            message: "no cached tasks".into(),
            ..ErrorPayload::default()
        }),
    });
    assert!(app.sign_in_required);
    assert!(app.banner.is_none());
    assert!(app.update(Msg::Action(Action::Add)).is_empty());
    assert_eq!(app.mode, Mode::Normal);
    assert!(app.update(Msg::Action(Action::Help)).is_empty());
    assert!(matches!(app.mode, Mode::Help { .. }));
}

#[test]
fn signed_out_with_a_ready_cache_keeps_tasks_available() {
    let mut app = App::new(UNICODE, clock())
        .with_sign_in_command(Some("ms-todo --instance scratch auth login".into()));
    let effects = app.update(Msg::Connected);
    answer_seed(&mut app, &effects[0], seed(scope_home(), home_tasks()));
    assert!(!app.sign_in_required);
    assert_eq!(app.tasks.len(), 4);
    app.update(Msg::Action(Action::Add));
    assert!(matches!(app.mode, Mode::Adding { .. }));
}

pub(crate) fn scope_home() -> Scope {
    Scope::List { id: "home".into() }
}

/// Answer `effect`, a seed request, with `seed`.
pub(crate) fn answer_seed(app: &mut App, effect: &Effect, seed: Seed) -> Vec<Effect> {
    assert!(matches!(effect.request, Request::Seed { .. }), "{effect:?}");
    app.update(Msg::Response {
        tag: effect.tag,
        result: Ok(ResponseData::Seed(seed)),
    })
}

pub(crate) fn act(app: &mut App, action: Action) -> Vec<Effect> {
    app.update(Msg::Action(action))
}

pub(crate) fn titles(app: &App) -> Vec<&str> {
    app.tasks.iter().map(|task| task.title.as_str()).collect()
}

pub(crate) fn applied(action: TaskAction, items: Vec<ms_todo_protocol::Entity>) -> ResponseData {
    ResponseData::Applied(ms_todo_protocol::Applied {
        op_id: "op-1".into(),
        action,
        list_ids: items.iter().map(|_| "home".to_owned()).collect(),
        items,
        rolled: Vec::new(),
        undoes: None,
        refused: Vec::new(),
    })
}

#[test]
fn connecting_asks_for_the_default_list_and_the_seed_fills_every_pane() {
    let mut app = App::new(UNICODE, clock());
    assert_eq!(app.connection, Connection::Connecting);
    let effects = app.update(Msg::Connected);
    assert_eq!(
        effects,
        [Effect {
            tag: Tag::Seed(1),
            request: Request::Seed {
                scope: None,
                search: None
            }
        }]
    );
    answer_seed(&mut app, &effects[0], seed(scope_home(), home_tasks()));
    assert!(app.seeded && app.lists_ready && app.tasks_ready);
    assert_eq!(app.wanted, Some(scope_home()));
    // Open tasks first, then completed ones.
    assert_eq!(
        titles(&app),
        ["Pay rent", "Call Sam", "Water plants", "Ship blueprint"]
    );
    // The sidebar follows: six views, then Tasks, then Home.
    assert_eq!(app.sidebar_index, 7);
    assert_eq!(app.lists.len(), 2);
    assert_eq!(app.counts.all, 3);
}

#[test]
fn j_k_g_and_capital_g_move_through_tasks_within_bounds() {
    let mut app = seeded();
    act(&mut app, Action::MoveUp);
    assert_eq!(app.task_index, 0);
    act(&mut app, Action::MoveDown);
    act(&mut app, Action::MoveDown);
    assert_eq!(
        app.selected().map(|t| t.title.as_str()),
        Some("Water plants")
    );
    act(&mut app, Action::JumpBottom);
    assert_eq!(app.task_index, 3);
    act(&mut app, Action::MoveDown);
    assert_eq!(app.task_index, 3);
    act(&mut app, Action::JumpTop);
    assert_eq!(app.task_index, 0);
}

#[test]
fn page_keys_move_by_the_visible_rows_and_stop_at_the_ends() {
    let mut app = seeded();
    app.update(Msg::Resize(Size::new(80, 8)));
    act(&mut app, Action::PageDown);
    assert_eq!(app.task_index, 2);
    act(&mut app, Action::PageDown);
    assert_eq!(app.task_index, 3);
    act(&mut app, Action::PageUp);
    assert_eq!(app.task_index, 1);
    act(&mut app, Action::PageUp);
    assert_eq!(app.task_index, 0);

    app.focus = Pane::Sidebar;
    act(&mut app, Action::PageUp);
    assert_eq!(app.sidebar_index, 5);
    act(&mut app, Action::PageDown);
    assert_eq!(app.sidebar_index, 7);
}

#[test]
fn page_keys_count_group_headings_in_completed_view() {
    let mut app = seeded();
    app.update(Msg::Resize(Size::new(80, 10)));
    app.shown = Some(Scope::Completed);
    app.tasks = (0..6)
        .map(|index| {
            Task::from_entity(&task(
                &format!("c{index}"),
                &format!("Task {index}"),
                json!({
                    "status": "completed",
                    "completedDateTime": {
                        "dateTime": format!("2026-09-{:02}T00:00:00.0000000", 24 - index),
                        "timeZone": "UTC"
                    }
                }),
            ))
            .expect("task")
        })
        .collect();
    act(&mut app, Action::PageDown);
    // Each task has a heading, so four screen rows move two tasks.
    assert_eq!(app.task_index, 2);
    act(&mut app, Action::PageUp);
    assert_eq!(app.task_index, 0);
}

#[test]
fn h_l_and_tab_move_between_panes() {
    let mut app = seeded();
    assert_eq!(app.focus, Pane::Tasks);
    act(&mut app, Action::FocusRight);
    assert_eq!(app.focus, Pane::Detail);
    act(&mut app, Action::FocusRight);
    assert_eq!(app.focus, Pane::Detail);
    act(&mut app, Action::FocusLeft);
    act(&mut app, Action::FocusLeft);
    assert_eq!(app.focus, Pane::Sidebar);
    assert_eq!(app.context(), Context::Sidebar);
    act(&mut app, Action::FocusNext);
    assert_eq!(app.focus, Pane::Tasks);
    act(&mut app, Action::FocusNext);
    act(&mut app, Action::FocusNext);
    assert_eq!(app.focus, Pane::Sidebar);
}

#[test]
fn moving_in_the_sidebar_seeds_the_new_scope_and_drops_stale_answers() {
    let mut app = seeded();
    act(&mut app, Action::FocusLeft);
    // From Home (7) up to Tasks, then to Completed.
    let first = act(&mut app, Action::MoveUp);
    let second = act(&mut app, Action::MoveUp);
    assert_eq!(
        second[0].request,
        Request::Seed {
            scope: Some(Scope::Completed),
            search: None
        }
    );
    // The answer for Tasks arrives after Completed was asked for: dropped.
    answer_seed(
        &mut app,
        &first[0],
        seed(Scope::List { id: "tasks".into() }, Vec::new()),
    );
    assert_eq!(app.shown, Some(scope_home()));
    let done = vec![task(
        "t3",
        "Ship blueprint",
        json!({ "status": "completed" }),
    )];
    answer_seed(&mut app, &second[0], seed(Scope::Completed, done));
    assert_eq!(app.shown, Some(Scope::Completed));
    assert_eq!(titles(&app), ["Ship blueprint"]);
    assert_eq!(app.sidebar_index, 5);
    // At the top, another k asks for nothing.
    act(&mut app, Action::JumpTop);
    assert!(act(&mut app, Action::MoveUp).is_empty());
}

#[test]
fn add_with_parsing_off_takes_the_text_literally_into_the_current_list() {
    let mut app = seeded();
    // The first `a` asks for the categories `@label` knows, once.
    let asked = act(&mut app, Action::Add);
    assert_eq!(asked[0].tag, Tag::Categories);
    assert_eq!(app.context(), Context::Adding);
    for ch in "Buy milk tomorrow #Home".chars() {
        app.update(Msg::Char(ch));
    }
    app.update(Msg::Action(Action::Backspace));
    act(&mut app, Action::ToggleParse);
    let effects = act(&mut app, Action::Submit);
    assert_eq!(app.mode, Mode::Normal);
    let Request::AddTask { task, dry_run, .. } = &effects[0].request else {
        unreachable!("{effects:?}");
    };
    assert_eq!(effects[0].tag, Tag::Write(Write::Add));
    assert!(!dry_run);
    assert_eq!(task.title, "Buy milk tomorrow #Hom");
    assert_eq!(task.list.as_deref(), Some("home"));
    assert_eq!((task.due.as_ref(), task.importance), (None, None));

    // Drawn from the answer at once, pending, and selected.
    let added = task_entity_pending("t9", "Buy milk tomorrow #Hom");
    app.update(Msg::Response {
        tag: Tag::Write(Write::Add),
        result: Ok(applied(TaskAction::Add, vec![added])),
    });
    assert_eq!(
        titles(&app),
        [
            "Pay rent",
            "Call Sam",
            "Water plants",
            "Buy milk tomorrow #Hom",
            "Ship blueprint"
        ]
    );
    assert_eq!(app.selected().map(|t| t.sync), Some(SyncMarker::Pending));
}

fn task_entity_pending(id: &str, title: &str) -> ms_todo_protocol::Entity {
    task(id, title, json!({ "sync_state": "pending" }))
}

#[test]
fn adding_from_a_view_gives_the_task_what_puts_it_in_the_view() {
    let mut app = seeded();
    app.shown = Some(Scope::Planned);
    app.wanted = app.shown.clone();
    act(&mut app, Action::Add);
    app.update(Msg::Char('x'));
    let effects = act(&mut app, Action::Submit);
    let Request::AddTask { task, .. } = &effects[0].request else {
        unreachable!("{effects:?}");
    };
    assert_eq!(task.list, None, "the default list");
    assert_eq!(task.due.as_deref(), Some("2026-09-24"));

    app.shown = Some(Scope::Important);
    app.wanted = app.shown.clone();
    act(&mut app, Action::Add);
    app.update(Msg::Char('y'));
    let effects = act(&mut app, Action::Submit);
    let Request::AddTask { task, .. } = &effects[0].request else {
        unreachable!("{effects:?}");
    };
    assert_eq!(task.importance, Some(ms_todo_protocol::Importance::High));
}

#[test]
fn an_empty_add_or_escape_adds_nothing() {
    let mut app = seeded();
    act(&mut app, Action::Add);
    app.update(Msg::Char(' '));
    assert!(act(&mut app, Action::Submit).is_empty());
    act(&mut app, Action::Add);
    app.update(Msg::Char('x'));
    assert!(act(&mut app, Action::Cancel).is_empty());
    assert_eq!(app.mode, Mode::Normal);
}

#[test]
fn adding_before_the_lists_have_synced_says_so() {
    let mut app = App::new(UNICODE, clock());
    act(&mut app, Action::Add);
    assert_eq!(app.mode, Mode::Normal);
    assert!(app.banner.is_some());
}

#[test]
fn x_completes_an_open_task_and_reopens_a_completed_one() {
    let mut app = seeded();
    let effects = act(&mut app, Action::ToggleComplete);
    assert_eq!(
        effects,
        [Effect {
            tag: Tag::Write(Write::Complete),
            request: Request::ChangeTasks {
                tasks: vec!["t1".into()],
                list: None,
                select: None,
                change: TaskChange::Complete,
                dry_run: false,
                op_id: None,
                idempotency_key: None,
            }
        }]
    );
    // The answer moves it to the completed ones, newest first; the cursor
    // stays put, on the next open task.
    let done = task(
        "t1",
        "Pay rent",
        json!({ "status": "completed", "sync_state": "pending" }),
    );
    app.update(Msg::Response {
        tag: Tag::Write(Write::Complete),
        result: Ok(applied(TaskAction::Complete, vec![done])),
    });
    assert_eq!(
        titles(&app),
        ["Call Sam", "Water plants", "Pay rent", "Ship blueprint"]
    );
    assert_eq!(app.task_index, 0);

    act(&mut app, Action::JumpBottom);
    let effects = act(&mut app, Action::ToggleComplete);
    let Request::ChangeTasks { tasks, change, .. } = &effects[0].request else {
        unreachable!("{effects:?}");
    };
    assert_eq!(
        (tasks.as_slice(), change),
        (["t3".to_owned()].as_slice(), &TaskChange::Reopen)
    );
    assert_eq!(effects[0].tag, Tag::Write(Write::Reopen));
}

#[test]
fn completing_in_a_view_of_open_tasks_takes_the_row_out() {
    let mut app = seeded();
    app.shown = Some(Scope::All);
    app.wanted = app.shown.clone();
    let done = task("t2", "Call Sam", json!({ "status": "completed" }));
    app.update(Msg::Response {
        tag: Tag::Write(Write::Complete),
        result: Ok(applied(TaskAction::Complete, vec![done])),
    });
    assert!(!titles(&app).contains(&"Call Sam"));
}

#[test]
fn d_asks_first_and_only_y_deletes() {
    let mut app = seeded();
    act(&mut app, Action::MoveDown);
    assert!(act(&mut app, Action::Delete).is_empty());
    assert_eq!(
        app.mode,
        Mode::ConfirmDelete {
            ids: vec!["t2".into()],
            what: "\"Call Sam\"".into()
        }
    );
    assert_eq!(app.context(), Context::Confirm);
    assert!(act(&mut app, Action::Cancel).is_empty());
    assert_eq!(app.mode, Mode::Normal);

    act(&mut app, Action::Delete);
    let effects = act(&mut app, Action::Confirm);
    let Request::ChangeTasks { tasks, change, .. } = &effects[0].request else {
        unreachable!("{effects:?}");
    };
    assert_eq!(
        (tasks.as_slice(), change),
        (["t2".to_owned()].as_slice(), &TaskChange::Delete)
    );
    app.update(Msg::Response {
        tag: Tag::Write(Write::Delete),
        result: Ok(applied(
            TaskAction::Delete,
            vec![task("t2", "Call Sam", json!({}))],
        )),
    });
    assert_eq!(titles(&app), ["Pay rent", "Water plants", "Ship blueprint"]);
}

#[test]
fn u_undoes_the_last_change_and_reads_the_cache_again() {
    let mut app = seeded();
    let effects = act(&mut app, Action::Undo);
    assert_eq!(
        effects,
        [Effect {
            tag: Tag::Undo,
            request: Request::Undo {
                target: None,
                copy: None,
                op_id: None,
                idempotency_key: None
            }
        }]
    );
    let effects = app.update(Msg::Response {
        tag: Tag::Undo,
        result: Ok(applied(TaskAction::Undo, Vec::new())),
    });
    assert!(matches!(effects[0].request, Request::Seed { .. }));
    assert_eq!(app.banner.as_ref().map(|b| b.text.as_str()), Some("Undone"));
}

#[test]
fn undoing_a_recurring_completion_picks_the_copy_to_delete() {
    let mut app = seeded();
    act(&mut app, Action::Undo);
    let candidates = vec![
        Candidate {
            id: "copy-1".into(),
            name: "Pay rent".into(),
            created_at: Some("2026-09-24T10:00:00Z".into()),
            list_id: Some("home".into()),
        },
        Candidate {
            id: "copy-2".into(),
            name: "Pay rent".into(),
            created_at: Some("2026-09-24T10:01:00Z".into()),
            list_id: Some("home".into()),
        },
    ];
    app.update(Msg::Response {
        tag: Tag::Undo,
        result: Err(ErrorPayload {
            kind: "invalid_input".into(),
            message: "name the copy".into(),
            candidates,
            undo_target: Some("op-7".into()),
            ..ErrorPayload::default()
        }),
    });
    assert_eq!(app.context(), Context::Picker);
    act(&mut app, Action::MoveDown);
    act(&mut app, Action::MoveDown);
    let effects = act(&mut app, Action::Submit);
    assert_eq!(
        effects[0].request,
        Request::Undo {
            target: Some("op-7".into()),
            copy: Some("copy-2".into()),
            op_id: None,
            idempotency_key: None
        }
    );
    assert_eq!(app.mode, Mode::Normal);

    // With no candidate yet, it just says why.
    act(&mut app, Action::Undo);
    app.update(Msg::Response {
        tag: Tag::Undo,
        result: Err(ErrorPayload {
            kind: "invalid_input".into(),
            message: "can't undo yet".into(),
            undo_target: Some("op-7".into()),
            ..ErrorPayload::default()
        }),
    });
    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(
        app.banner.as_ref().map(|b| b.text.as_str()),
        Some("can't undo yet")
    );
}

#[test]
fn slash_filters_as_you_type_and_escape_clears_it() {
    let mut app = seeded();
    assert!(act(&mut app, Action::Filter).is_empty());
    let effects = app.update(Msg::Char('r'));
    assert_eq!(
        effects[0].request,
        Request::Seed {
            scope: Some(scope_home()),
            search: Some("r*".into())
        }
    );
    let effects = app.update(Msg::Char('e'));
    answer_seed(
        &mut app,
        &effects[0],
        seed(scope_home(), vec![task("t1", "Pay rent", json!({}))]),
    );
    assert_eq!(titles(&app), ["Pay rent"]);
    act(&mut app, Action::Submit);
    assert_eq!(
        (app.mode.clone(), app.filter.as_deref()),
        (Mode::Normal, Some("re"))
    );

    // Esc in the list drops it and reads the whole list again.
    let effects = act(&mut app, Action::Clear);
    assert_eq!(
        effects[0].request,
        Request::Seed {
            scope: Some(scope_home()),
            search: None
        }
    );
    assert_eq!(app.filter, None);
}

#[test]
fn a_filter_mid_typing_that_cant_be_searched_keeps_the_last_results() {
    let mut app = seeded();
    act(&mut app, Action::Filter);
    let effects = app.update(Msg::Char('"'));
    app.update(Msg::Response {
        tag: effects[0].tag,
        result: Err(ErrorPayload {
            kind: "invalid_input".into(),
            message: "unclosed quote".into(),
            ..ErrorPayload::default()
        }),
    });
    assert_eq!(app.filter_error.as_deref(), Some("unclosed quote"));
    assert_eq!(app.tasks.len(), 4);
    assert!(app.banner.is_none());
}

#[test]
fn the_filter_matches_the_word_being_typed_as_a_prefix() {
    assert_eq!(search_query("mil"), "mil*");
    assert_eq!(search_query("milk "), "milk");
    assert_eq!(search_query("milk OR"), "milk OR");
    assert_eq!(search_query("insur*"), "insur*");
    assert_eq!(search_query("\"car tax\""), "\"car tax\"");
}

#[test]
fn r_syncs_question_mark_helps_and_q_quits() {
    let mut app = seeded();
    assert_eq!(
        act(&mut app, Action::Sync),
        [Effect {
            tag: Tag::Sync,
            request: Request::Sync { wait: false }
        }]
    );
    act(&mut app, Action::Help);
    assert_eq!(app.context(), Context::Help);
    act(&mut app, Action::Cancel);
    assert_eq!(app.mode, Mode::Normal);
    assert!(!app.should_quit);
    act(&mut app, Action::Quit);
    assert!(app.should_quit);
}

fn help_scroll(app: &App) -> Option<u16> {
    match app.mode {
        Mode::Help { scroll } => Some(scroll),
        _ => None,
    }
}

#[test]
fn help_scrolls_no_further_than_its_last_row() {
    let small = ratatui::layout::Size::new(80, 24);
    let page = crate::help::layout(small);
    let (max, step) = (page.max_scroll(), page.page());
    assert!(max > step, "help is taller than two screens at 80x24");
    let mut app = seeded();
    app.update(Msg::Resize(small));
    act(&mut app, Action::Help);
    assert_eq!(help_scroll(&app), Some(0));
    act(&mut app, Action::MoveUp);
    assert_eq!(help_scroll(&app), Some(0), "no further up than the top");
    act(&mut app, Action::MoveDown);
    assert_eq!(help_scroll(&app), Some(1));
    act(&mut app, Action::PageUp);
    assert_eq!(help_scroll(&app), Some(0));
    act(&mut app, Action::PageDown);
    assert_eq!(help_scroll(&app), Some(step));
    act(&mut app, Action::JumpBottom);
    assert_eq!(help_scroll(&app), Some(max));
    act(&mut app, Action::MoveDown);
    act(&mut app, Action::PageDown);
    assert_eq!(
        help_scroll(&app),
        Some(max),
        "no further down than the last row"
    );
    act(&mut app, Action::JumpTop);
    assert_eq!(help_scroll(&app), Some(0));
    // Growing the terminal until it all fits takes the scroll with it.
    act(&mut app, Action::JumpBottom);
    app.update(Msg::Resize(ratatui::layout::Size::new(200, 200)));
    assert_eq!(help_scroll(&app), Some(0));
    act(&mut app, Action::MoveDown);
    assert_eq!(help_scroll(&app), Some(0));
    // Closed and opened again, it starts at the top.
    app.update(Msg::Resize(small));
    act(&mut app, Action::JumpBottom);
    act(&mut app, Action::Cancel);
    assert_eq!(app.mode, Mode::Normal);
    act(&mut app, Action::Help);
    assert_eq!(help_scroll(&app), Some(0));
}

#[test]
fn changes_reseed_once_per_burst() {
    let mut app = seeded();
    let changed = || Msg::Event(Event::EntityChanged(EntityChanged::default()));
    let first = app.update(changed());
    assert_eq!(first.len(), 1);
    // Two more while it's in flight: one more seed after it lands.
    assert!(app.update(changed()).is_empty());
    assert!(app.update(Msg::Event(Event::ResyncNeeded)).is_empty());
    let again = answer_seed(&mut app, &first[0], seed(scope_home(), home_tasks()));
    assert_eq!(again.len(), 1);
    assert!(answer_seed(&mut app, &again[0], seed(scope_home(), home_tasks())).is_empty());
}

#[test]
fn a_reseed_keeps_the_selected_task() {
    let mut app = seeded();
    act(&mut app, Action::MoveDown);
    let effects = app.update(Msg::Event(Event::ResyncNeeded));
    // "Call Sam" moved to the top on another device.
    let mut tasks = home_tasks();
    tasks.swap(0, 1);
    answer_seed(&mut app, &effects[0], seed(scope_home(), tasks));
    assert_eq!(app.selected().map(|t| t.id.as_str()), Some("t2"));
}

#[test]
fn a_rejected_write_shows_a_banner_and_the_row_rolls_back() {
    let mut app = seeded();
    let effects = app.update(Msg::Event(Event::WriteRejected(WriteRejected {
        op_id: "op-1".into(),
        task_id: "t4".into(),
        error: OpError {
            kind: "rejected".into(),
            message: "the task is too long".into(),
        },
    })));
    let banner = app.banner.clone().expect("banner");
    assert_eq!(banner.level, Level::Error);
    assert!(banner.text.contains("the task is too long"));
    assert_eq!(
        app.rejections.get("t4").map(String::as_str),
        Some("the task is too long")
    );
    assert!(matches!(effects[0].request, Request::Seed { .. }));
    // The banner goes after its ticks.
    for _ in 0..BANNER_TICKS {
        app.update(Msg::Tick(clock()));
    }
    assert!(app.banner.is_none());
}

#[test]
fn before_the_first_sync_the_list_is_syncing_not_empty() {
    let mut app = App::new(UNICODE, clock());
    let effects = app.update(Msg::Connected);
    let mut first = seed(scope_home(), Vec::new());
    first.scope = None;
    first.lists_sync.state = SyncState::Initial;
    first.sync.state = SyncState::Initial;
    first.activity.in_progress = true;
    answer_seed(&mut app, &effects[0], first);
    assert!(app.seeded && !app.lists_ready && !app.tasks_ready);
    // A pass that finishes while nothing is ready asks again.
    let done = SyncActivity {
        generation: 1,
        in_progress: false,
        last_finished_at: Some(clock().unix()),
        last_error: None,
    };
    let effects = app.update(Msg::Event(Event::SyncState(done.clone())));
    assert_eq!(effects.len(), 1);
    assert_eq!(app.activity, done);
}

#[test]
fn a_deleted_list_falls_back_to_the_default_one() {
    let mut app = seeded();
    let effects = app.update(Msg::Event(Event::ResyncNeeded));
    let effects = app.update(Msg::Response {
        tag: effects[0].tag,
        result: Err(ErrorPayload {
            kind: "not_found".into(),
            message: "no list".into(),
            ..ErrorPayload::default()
        }),
    });
    assert_eq!(
        effects[0].request,
        Request::Seed {
            scope: None,
            search: None
        }
    );
}

#[test]
fn a_lost_connection_is_shown_and_reconnecting_seeds_again() {
    let mut app = seeded();
    app.update(Msg::Disconnected("the daemon closed the connection".into()));
    assert!(matches!(app.connection, Connection::Lost(_)));
    let effects = app.update(Msg::Connected);
    assert_eq!(app.connection, Connection::Connected);
    assert_eq!(
        effects[0].request,
        Request::Seed {
            scope: Some(scope_home()),
            search: None
        }
    );
}

#[test]
fn a_failed_write_says_why() {
    let mut app = seeded();
    app.update(Msg::Response {
        tag: Tag::Write(Write::Complete),
        result: Err(ErrorPayload {
            kind: "daemon_unavailable".into(),
            message: "the change may or may not have been made".into(),
            ..ErrorPayload::default()
        }),
    });
    assert_eq!(
        app.banner.map(|b| (b.level, b.text)),
        Some((
            Level::Error,
            "the change may or may not have been made".to_owned()
        ))
    );
}

#[test]
fn the_views_are_read_ahead_and_a_scope_seen_before_paints_at_once() {
    let mut app = App::new(UNICODE, clock());
    let effects = app.update(Msg::Connected);
    let prefetches = answer_seed(&mut app, &effects[0], seed(scope_home(), home_tasks()));
    let scopes: Vec<_> = prefetches
        .iter()
        .map(|effect| match &effect.request {
            Request::Seed { scope, .. } => scope.clone(),
            other => unreachable!("{other:?}"),
        })
        .collect();
    assert_eq!(
        scopes,
        [
            Some(Scope::MyDay),
            Some(Scope::Important),
            Some(Scope::Planned),
            Some(Scope::All),
            Some(Scope::Assigned),
            Some(Scope::Completed)
        ]
    );
    let done = vec![task(
        "t3",
        "Ship blueprint",
        json!({ "status": "completed" }),
    )];
    app.update(Msg::Response {
        tag: prefetches[5].tag,
        result: Ok(ResponseData::Seed(seed(Scope::Completed, done))),
    });
    // Read ahead, not drawn.
    assert_eq!(app.shown, Some(scope_home()));

    // Home (5) up to Tasks (4, never read) and on to Completed (3).
    act(&mut app, Action::FocusLeft);
    act(&mut app, Action::MoveUp);
    assert!(!app.painted_from_cache);
    assert_eq!(app.shown, Some(scope_home()), "Tasks waits for its seed");
    let effects = act(&mut app, Action::MoveUp);
    assert!(app.painted_from_cache);
    assert_eq!(app.shown, Some(Scope::Completed));
    assert_eq!(titles(&app), ["Ship blueprint"]);
    // And its fresh seed is still asked for.
    assert_eq!(
        effects[0].request,
        Request::Seed {
            scope: Some(Scope::Completed),
            search: None
        }
    );
}

#[test]
fn after_switching_to_a_scope_not_loaded_yet_no_action_takes_the_old_rows() {
    let mut app = seeded();
    act(&mut app, Action::FocusLeft);
    // Home (5) up to Tasks (4), which nothing has read yet.
    let pending = act(&mut app, Action::MoveUp);
    act(&mut app, Action::FocusRight);
    assert!(app.loading());
    assert_eq!(app.selected(), None, "Home's rows aren't Tasks'");
    for action in [Action::ToggleComplete, Action::Delete, Action::Add] {
        assert!(act(&mut app, action).is_empty(), "{action:?}");
        assert_eq!(app.mode, Mode::Normal, "{action:?}");
        assert!(
            app.banner
                .as_ref()
                .is_some_and(|banner| banner.text.starts_with("Still loading")),
            "{action:?}"
        );
    }
    // An event mid-switch rereads the scope being switched to, not Home.
    let reread = app.update(Msg::Event(Event::ResyncNeeded));
    assert!(reread.is_empty(), "coalesced behind the seed in flight");

    let tasks = Scope::List { id: "tasks".into() };
    let mut milk = task("m1", "Buy milk", json!({}));
    milk.insert("list_id".into(), json!("tasks"));
    let again = answer_seed(&mut app, &pending[0], seed(tasks.clone(), vec![milk]));
    assert!(!app.loading());
    assert_eq!(app.selected().map(|t| t.id.as_str()), Some("m1"));
    assert_eq!(
        again[0].request,
        Request::Seed {
            scope: Some(tasks),
            search: None
        }
    );
    let effects = act(&mut app, Action::ToggleComplete);
    assert!(
        matches!(&effects[0].request, Request::ChangeTasks { tasks, .. } if tasks == &["m1".to_owned()])
    );
}

#[test]
fn a_refresh_that_reorders_rows_keeps_the_selection_on_the_same_task() {
    let mut app = seeded();
    act(&mut app, Action::MoveDown);
    act(&mut app, Action::MoveDown);
    assert_eq!(app.selected().map(|t| t.id.as_str()), Some("t4"));
    // Elsewhere, "Water plants" moved to the top and a task was added.
    let mut tasks = home_tasks();
    let water = tasks.remove(3);
    tasks.insert(0, water);
    tasks.insert(1, task("t5", "New one", json!({})));
    let effects = app.update(Msg::Event(Event::ResyncNeeded));
    answer_seed(&mut app, &effects[0], seed(scope_home(), tasks));
    assert_eq!(app.task_index, 0);
    assert_eq!(app.selected().map(|t| t.id.as_str()), Some("t4"));
}

#[test]
fn the_window_title_names_the_app_and_the_view() {
    let mut app = seeded();
    assert_eq!(app.window_title(), "ms-todo \u{2014} Home");
    act(&mut app, Action::FocusLeft);
    act(&mut app, Action::JumpTop);
    // My Day's title has its day.
    assert_eq!(
        app.window_title(),
        "ms-todo \u{2014} My Day \u{b7} Thu 24 Sep"
    );
}

#[test]
fn a_list_name_cannot_put_escape_sequences_in_the_window_title() {
    let mut app = seeded();
    app.lists[1].name = "Home\x1b]52;c;aGk=\x07\u{9b}\n".into();
    let title = app.window_title();
    assert!(!title.contains(char::is_control), "{title:?}");
    assert_eq!(title, "ms-todo \u{2014} Home]52;c;aGk=");
}
