//! Quick add through the real binary and daemon, against a fake Graph:
//! what `tasks add` reads out of its text and sends, flags winning over
//! it, `--no-parse`, `tasks parse`, and `@label` against the user's
//! categories. Dates are relative to the real clock, in London (the test
//! environment's `TZ`).

mod support;

use chrono::{Datelike, Days, Months, NaiveDate};
use serde_json::{Value, json};
use support::Env;
use support::fake_graph::{FakeGraph, list};
use wiremock::matchers::{method, path};
use wiremock::{Mock, Request, ResponseTemplate};

async fn graph(env: &mut Env) -> FakeGraph {
    let graph = FakeGraph::start(
        env,
        vec![
            list("L-tasks", "Tasks", "defaultList"),
            list("L-fin", "Finances", "none"),
        ],
    )
    .await;
    graph.edit(|data| {
        data.tasks.insert("L-tasks".into(), Vec::new());
        data.tasks.insert("L-fin".into(), Vec::new());
    });
    // Creates answer with what was sent.
    Mock::given(method("POST"))
        .and(wiremock::matchers::path_regex(
            r"^/v1\.0/me/todo/lists/[^/]+/tasks$",
        ))
        .respond_with(|request: &Request| {
            let mut created: Value = serde_json::from_slice(&request.body).expect("json body");
            created["id"] = json!("T-new");
            ResponseTemplate::new(201).set_body_json(created)
        })
        .mount(&graph.server)
        .await;
    graph
}

async fn categories(graph: &FakeGraph, names: &[&str]) {
    let value: Vec<Value> = names
        .iter()
        .map(|name| json!({ "id": format!("C-{name}"), "displayName": name, "color": "preset0" }))
        .collect();
    Mock::given(method("GET"))
        .and(path("/v1.0/me/outlook/masterCategories"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "value": value })))
        .mount(&graph.server)
        .await;
}

/// The one task create Graph got, and the list it went to.
async fn created(env: &Env, graph: &FakeGraph) -> (String, Value) {
    env.settled();
    let posts: Vec<Request> = graph
        .writes()
        .await
        .into_iter()
        .filter(|request| request.url.path().ends_with("/tasks"))
        .collect();
    assert_eq!(posts.len(), 1, "{posts:?}");
    let body = serde_json::from_slice(&posts[0].body).expect("json");
    (posts[0].url.path().to_owned(), body)
}

fn next_first(env: &Env) -> NaiveDate {
    let today = env.today();
    if today.day() == 1 {
        return today;
    }
    today
        .with_day(1)
        .and_then(|first| first.checked_add_months(Months::new(1)))
        .expect("a date")
}

fn day(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

#[tokio::test]
async fn the_demo_files_a_recurring_task_with_its_list_importance_and_reminder() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;

    let added = env.json(&["tasks", "add", "Pay rent every 1st #Finances p1 9am"]);
    assert_eq!(added["items"][0]["title"], "Pay rent");

    let (path, body) = created(&env, &graph).await;
    assert_eq!(path, "/v1.0/me/todo/lists/L-fin/tasks");
    let first = day(next_first(&env));
    assert_eq!(body["title"], "Pay rent");
    assert_eq!(body["importance"], "high");
    assert_eq!(
        body["dueDateTime"],
        json!({ "dateTime": format!("{first}T00:00:00"), "timeZone": "Europe/London" })
    );
    assert_eq!(body["isReminderOn"], true);
    assert_eq!(
        body["reminderDateTime"],
        json!({ "dateTime": format!("{first}T09:00:00"), "timeZone": "Europe/London" })
    );
    assert_eq!(
        body["recurrence"],
        json!({
            "pattern": { "type": "absoluteMonthly", "interval": 1, "dayOfMonth": 1 },
            "range": { "type": "noEnd", "startDate": first, "recurrenceTimeZone": "Europe/London" }
        })
    );
}

#[tokio::test]
async fn a_relative_date_is_the_due_date_and_leaves_the_title() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    env.json(&["tasks", "add", "Call mum in 2 days"]);
    let (path, body) = created(&env, &graph).await;
    assert_eq!(
        path, "/v1.0/me/todo/lists/L-tasks/tasks",
        "Tasks by default"
    );
    assert_eq!(body["title"], "Call mum");
    let due = day(env.today().checked_add_days(Days::new(2)).expect("a date"));
    assert_eq!(body["dueDateTime"]["dateTime"], format!("{due}T00:00:00"));
    assert!(body.get("reminderDateTime").is_none());
}

#[tokio::test]
async fn flags_win_over_what_the_text_says() {
    let mut env = Env::new();
    let _graph = graph(&mut env).await;
    let plan = env.json(&[
        "tasks",
        "add",
        "Gym every mon #Finances p1 tomorrow 7am",
        "--list",
        "Tasks",
        "--due",
        "2030-01-07",
        "--reminder",
        "2030-01-07T06:30",
        "--importance",
        "low",
        "--dry-run",
    ]);
    assert_eq!(plan["list"]["name"], "Tasks");
    let changes = &plan["changes"];
    assert_eq!(changes["title"], "Gym");
    assert_eq!(changes["importance"], "low");
    assert_eq!(changes["dueDateTime"]["dateTime"], "2030-01-07T00:00:00");
    assert_eq!(
        changes["reminderDateTime"]["dateTime"],
        "2030-01-07T06:30:00"
    );
    // The recurrence starts on the due date the flag gave.
    assert_eq!(changes["recurrence"]["range"]["startDate"], "2030-01-07");
    assert_eq!(
        changes["recurrence"]["pattern"]["daysOfWeek"],
        json!(["monday"])
    );
}

#[tokio::test]
async fn no_parse_takes_the_text_as_the_title() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    env.json(&[
        "tasks",
        "add",
        "Email Friday's report every 1st #Finances p1",
        "--no-parse",
    ]);
    let (path, body) = created(&env, &graph).await;
    assert_eq!(path, "/v1.0/me/todo/lists/L-tasks/tasks");
    assert_eq!(
        body["title"],
        "Email Friday's report every 1st #Finances p1"
    );
    for field in ["recurrence", "importance", "dueDateTime", "categories"] {
        assert!(body.get(field).is_none(), "{field}: {body}");
    }
}

#[tokio::test]
async fn nothing_left_for_the_title_is_refused() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    let error = env.failure(&["tasks", "add", "tomorrow p1"], 2);
    assert_eq!(error["error"]["kind"], "invalid_input");
    assert!(
        error["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("--no-parse")),
        "{error}"
    );
    assert!(graph.writes().await.is_empty());
}

#[tokio::test]
async fn parse_shows_the_reading_and_writes_nothing() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    categories(&graph, &["Errands"]).await;
    let parsed = env.json(&["tasks", "parse", "Buy milk #fin @errands @new p2 +myday"]);
    assert_eq!(parsed["schema_version"], 2);
    assert_eq!(parsed["input"], "Buy milk #fin @errands @new p2 +myday");
    assert_eq!(parsed["title"], "Buy milk");
    assert_eq!(parsed["list"]["name"], "Finances");
    assert_eq!(parsed["importance"], "normal");
    assert_eq!(parsed["priority"], 2);
    assert_eq!(parsed["categories"], json!(["Errands", "new"]));
    assert_eq!(parsed["my_day"], true);
    let kinds: Vec<&str> = parsed["spans"]
        .as_array()
        .expect("spans")
        .iter()
        .filter_map(|span| span["kind"].as_str())
        .collect();
    assert_eq!(kinds, ["list", "label", "label", "priority", "my_day"]);
    let warnings = parsed["warnings"].to_string();
    assert!(
        warnings.contains("@new isn't one of your categories"),
        "{warnings}"
    );
    assert!(!warnings.contains("My Day"), "{warnings}");
    assert!(graph.writes().await.is_empty());

    // A person's view of it.
    let table = env
        .cmd()
        .args([
            "--format",
            "table",
            "tasks",
            "parse",
            "Pay rent every 1st #Finances p1 9am",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let table = String::from_utf8(table).expect("utf-8");
    assert!(table.contains("Finances"), "{table}");
    assert!(table.contains("every month on the 1st"), "{table}");
    assert!(table.contains("remind 09:00"), "{table}");
}

#[tokio::test]
async fn create_categories_adds_a_missing_one_before_the_task() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    categories(&graph, &["Errands"]).await;
    Mock::given(method("POST"))
        .and(path("/v1.0/me/outlook/masterCategories"))
        .respond_with(|request: &Request| {
            let mut created: Value = serde_json::from_slice(&request.body).expect("json body");
            created["id"] = json!("C-new");
            ResponseTemplate::new(201).set_body_json(created)
        })
        .mount(&graph.server)
        .await;

    env.json(&[
        "tasks",
        "add",
        "Buy milk @errands @Garden",
        "--create-categories",
    ]);
    env.settled();
    let writes = graph.writes().await;
    let created_categories: Vec<Value> = writes
        .iter()
        .filter(|request| request.url.path() == "/v1.0/me/outlook/masterCategories")
        .map(|request| serde_json::from_slice(&request.body).expect("json"))
        .collect();
    assert_eq!(created_categories, [json!({ "displayName": "Garden" })]);
    let (_, body) = created(&env, &graph).await;
    assert_eq!(body["categories"], json!(["Errands", "Garden"]));

    // Without the flag, nothing but the task is written.
    let mut env = Env::new();
    let graph = self::graph(&mut env).await;
    categories(&graph, &[]).await;
    env.json(&["tasks", "add", "Weed @Garden"]);
    let (_, body) = created(&env, &graph).await;
    assert_eq!(body["categories"], json!(["Garden"]));
}

#[tokio::test]
async fn clearing_flags_clear_what_the_text_set() {
    let mut env = Env::new();
    let _graph = graph(&mut env).await;
    let plan = env.json(&[
        "tasks",
        "add",
        "Dentist tomorrow 9am",
        "--due",
        "-",
        "--reminder",
        "-",
        "--dry-run",
    ]);
    let changes = &plan["changes"];
    assert_eq!(changes["title"], "Dentist");
    assert!(changes.get("dueDateTime").is_none(), "{changes}");
    assert!(changes.get("reminderDateTime").is_none(), "{changes}");

    // A reminder from the text survives clearing only the due date.
    let plan = env.json(&[
        "tasks",
        "add",
        "Dentist tomorrow 9am",
        "--due",
        "-",
        "--dry-run",
    ]);
    assert!(plan["changes"].get("dueDateTime").is_none());
    assert_eq!(plan["changes"]["isReminderOn"], true);
}

#[tokio::test]
async fn clearing_the_due_date_of_a_recurrence_is_refused() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    for text in ["Gym every mon", "Trip start mon"] {
        let error = env.failure(&["tasks", "add", text, "--due", "-"], 2);
        assert_eq!(error["error"]["kind"], "invalid_input");
        let message = error["error"]["message"].as_str().unwrap_or_default();
        assert!(message.contains("--due -"), "{message}");
    }
    assert!(graph.writes().await.is_empty());
}

#[tokio::test]
async fn a_list_name_two_lists_share_files_nowhere_it_names() {
    let mut env = Env::new();
    let graph = FakeGraph::start(
        &mut env,
        vec![
            list("L-tasks", "Tasks", "defaultList"),
            list("L-home-1", "Home", "none"),
            list("L-home-2", "Home", "none"),
        ],
    )
    .await;
    graph.edit(|data| {
        for id in ["L-tasks", "L-home-1", "L-home-2"] {
            data.tasks.insert(id.into(), Vec::new());
        }
    });
    let parsed = env.json(&["tasks", "parse", "Fix tap #Home"]);
    assert_eq!(parsed["list"], Value::Null);
    assert_eq!(parsed["title"], "Fix tap #Home");
    assert!(
        parsed["warnings"]
            .to_string()
            .contains("2 lists are called Home")
    );
    let plan = env.json(&["tasks", "add", "Fix tap #Home", "--dry-run"]);
    assert_eq!(plan["list"]["name"], "Tasks", "the default list");
}

fn write_config(env: &Env, contents: &str) {
    let dir = env.home.path().join("config").join("ms-todo");
    std::fs::create_dir_all(&dir).expect("config dir");
    std::fs::write(dir.join("config.toml"), contents).expect("config");
}

/// The next `month`/`day` on or after today.
fn next_day_of_year(env: &Env, month: u32, day_of_month: u32) -> NaiveDate {
    let today = env.today();
    NaiveDate::from_ymd_opt(today.year(), month, day_of_month)
        .filter(|date| *date >= today)
        .or_else(|| NaiveDate::from_ymd_opt(today.year() + 1, month, day_of_month))
        .expect("a date")
}

#[tokio::test]
async fn dates_in_config_toml_order_slashed_dates_and_start_weeks() {
    let mut env = Env::new();
    let _graph = graph(&mut env).await;
    // Day first by default.
    let parsed = env.json(&["tasks", "parse", "Mot 12/10"]);
    assert_eq!(parsed["due"], day(next_day_of_year(&env, 10, 12)));

    write_config(
        &env,
        "[dates]\ndate_order = \"mdy\"\nweek_start = \"sunday\"\n",
    );
    let parsed = env.json(&["tasks", "parse", "Mot 12/10 every other week"]);
    assert_eq!(
        parsed["recurrence"]["pattern"]["firstDayOfWeek"], "sunday",
        "{parsed}"
    );
    let parsed = env.json(&["tasks", "parse", "Mot 12/10"]);
    assert_eq!(parsed["due"], day(next_day_of_year(&env, 12, 10)));
    // The flags read the same way.
    let plan = env.json(&["tasks", "add", "Mot", "--due", "12/10", "--dry-run"]);
    assert_eq!(
        plan["changes"]["dueDateTime"]["dateTime"],
        format!("{}T00:00:00", day(next_day_of_year(&env, 12, 10)))
    );
    // Next week starts on the Sunday after today.
    let today = env.today();
    let sunday = today
        .checked_add_days(Days::new(u64::from(
            7 - today.weekday().num_days_from_sunday(),
        )))
        .expect("a date");
    let parsed = env.json(&["tasks", "parse", "Plan next week"]);
    assert_eq!(parsed["due"], day(sunday));

    // A typo is refused where a phrase is read, naming the setting.
    write_config(&env, "[dates]\ndate_order = \"ymd\"\n");
    let error = env.failure(&["tasks", "parse", "Mot 12/10"], 2);
    let message = error["error"]["message"].as_str().expect("message");
    assert!(message.contains("dates.date_order"), "{message}");
    env.cmd()
        .args(["tasks", "add", "Mot", "--due", "fri", "--dry-run"])
        .assert()
        .code(2);
}
