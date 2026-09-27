//! Nag reminders (rung 9b, D-063): an open task set to nag shows a
//! notification on this machine once its reminder time passes, then every
//! so many minutes until it's completed, the nag is turned off, or the
//! task is deleted. The setting syncs in our extension; the notifications
//! are this daemon's alone, and nothing else is written to Microsoft.
//!
//! When each task last nagged is kept in the store's settings, so a
//! restarted daemon owes each task at most one notification, not a burst.

mod config;
mod notify;
mod schedule;
pub(crate) mod setting;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Local, TimeZone, Utc};
use ms_todo_core::ErrorKind;
use ms_todo_protocol::{ErrorPayload, NagStatus, ResponseData};
use ms_todo_store::TaskRow;

use crate::handlers::{State, error_payload, store_error};
use crate::task_fields::reminder_at;

pub(crate) use config::Config;
use notify::Notifier;
use setting::nag_of;

/// When each task last nagged: local ID to Unix milliseconds, as JSON.
const LAST_NAGGED: &str = "nag.last_notified";

pub(crate) struct Nagger {
    config: Config,
    notifier: Notifier,
    /// The last notification that failed, until one works again.
    failure: Mutex<Option<String>>,
    /// Held while the saved last-nagged times are read and written, so a
    /// tick can't put back a time `forget` just removed.
    saving: tokio::sync::Mutex<()>,
}

impl Nagger {
    pub fn load(config_file: &std::path::Path) -> Self {
        let config = Config::load(config_file);
        let notifier = Notifier::for_this_system(config::notify_file());
        Self {
            config,
            notifier,
            failure: Mutex::new(None),
            saving: tokio::sync::Mutex::new(()),
        }
    }

    fn set_failure(&self, failure: Option<String>) {
        if let Ok(mut slot) = self.failure.lock() {
            *slot = failure;
        }
    }

    fn failure(&self) -> Option<String> {
        self.failure.lock().ok().and_then(|slot| slot.clone())
    }
}

/// The nagger's loop: twice a nag-minute, for as long as the daemon runs.
pub(crate) async fn run(state: Arc<State>) {
    let nagger = &state.nag;
    if !nagger.config.enabled {
        return;
    }
    if let Some(why) = nagger.notifier.unavailable() {
        eprintln!("ms-todo daemon: nag: {why}");
        return;
    }
    loop {
        if let Err(error) = tick(&state).await {
            eprintln!("ms-todo daemon: nag: {}", error.message);
        }
        tokio::time::sleep(nagger.config.minute / 2).await;
    }
}

async fn tick(state: &State) -> Result<(), ErrorPayload> {
    let nagger = &state.nag;
    // Nothing shows in quiet hours, so nothing needs reading either.
    if nagger
        .config
        .quiet_hours
        .is_some_and(|quiet| quiet.contains(Local::now().time()))
    {
        return Ok(());
    }
    let _saving = nagger.saving.lock().await;
    let rows = state.store.nagging_tasks().await.map_err(store_error)?;
    let mut last = last_nagged(state).await?;
    let before = last.clone();
    let now = Utc::now();
    let today = crate::deferral::today();
    let mut lists: Option<HashMap<String, String>> = None;
    for row in &rows {
        // Deferred or Someday is out of sight (D-061), so it waits too.
        if crate::deferral::hidden(row, today)
            || !due(nagger, row, now, last.get(&row.local_id).copied())
        {
            continue;
        }
        if lists.is_none() {
            let rows = state.store.lists().await.map_err(store_error)?;
            lists = Some(
                rows.into_iter()
                    .map(|list| (list.local_id, list.display_name))
                    .collect(),
            );
        }
        let list = lists
            .as_ref()
            .and_then(|lists| lists.get(&row.list_local_id))
            .map_or("", String::as_str);
        match nagger.notifier.show(&row.title, list).await {
            Ok(()) => {
                nagger.set_failure(None);
                eprintln!("ms-todo daemon: nag: notified {}", row.local_id);
            }
            Err(why) => {
                eprintln!("ms-todo daemon: nag: {} not shown: {why}", row.local_id);
                nagger.set_failure(Some(why));
            }
        }
        // Even when it failed: the next try waits an interval, rather
        // than filling the log twice a minute.
        last.insert(row.local_id.clone(), now.timestamp_millis());
    }
    // Forget tasks that stopped nagging, so starting again nags at once.
    let nagging: HashSet<&str> = rows.iter().map(|row| row.local_id.as_str()).collect();
    last.retain(|id, _| nagging.contains(id.as_str()));
    if last != before {
        let json = serde_json::to_string(&last).unwrap_or_default();
        state
            .store
            .set_settings(&[(LAST_NAGGED, &json)])
            .await
            .map_err(store_error)?;
    }
    Ok(())
}

/// Whether `row` is due a notification at `now`, having last nagged at
/// `last` (Unix milliseconds).
fn due(nagger: &Nagger, row: &TaskRow, now: DateTime<Utc>, last: Option<i64>) -> bool {
    let (Some(minutes), Some(reminder)) = (nag_of(row), reminder_at(&row.raw)) else {
        return false;
    };
    // The earlier of a time the clocks went back over, as Graph would.
    let Some(reminder) = Local.from_local_datetime(&reminder).earliest() else {
        return false;
    };
    let Ok(every) = chrono::Duration::from_std(nagger.config.minute * minutes) else {
        return false;
    };
    let last = last.and_then(DateTime::from_timestamp_millis);
    schedule::due(now, reminder.with_timezone(&Utc), every, last)
}

/// Forget when `ids` last nagged, so a nag just set nags as soon as its
/// reminder has passed. The tick's own pruning can't do this: it skips
/// quiet hours, and misses a nag turned off and on between two ticks.
pub(crate) async fn forget(state: &State, ids: &[&str]) -> Result<(), ErrorPayload> {
    let _saving = state.nag.saving.lock().await;
    let mut last = last_nagged(state).await?;
    let before = last.len();
    last.retain(|id, _| !ids.contains(&id.as_str()));
    if last.len() == before {
        return Ok(());
    }
    let json = serde_json::to_string(&last).unwrap_or_default();
    state
        .store
        .set_settings(&[(LAST_NAGGED, &json)])
        .await
        .map_err(store_error)
}

async fn last_nagged(state: &State) -> Result<HashMap<String, i64>, ErrorPayload> {
    let saved = state
        .store
        .setting(LAST_NAGGED)
        .await
        .map_err(store_error)?;
    // Unreadable is forgotten: at worst, one notification each too soon.
    Ok(saved
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default())
}

pub(crate) async fn status(state: &State) -> Result<NagStatus, ErrorPayload> {
    let nagger = &state.nag;
    let count = state.store.nagging_count().await.map_err(store_error)?;
    let unavailable = nagger.notifier.unavailable().map(str::to_owned);
    let problem = nagger
        .config
        .problem
        .clone()
        .or_else(|| unavailable.filter(|_| nagger.config.enabled))
        .or_else(|| nagger.failure());
    Ok(NagStatus {
        active: nagger.config.enabled && nagger.notifier.name().is_some(),
        notifier: nagger.notifier.name().map(str::to_owned),
        quiet_hours: nagger.config.quiet_hours.map(schedule::QuietHours::label),
        count,
        problem,
    })
}

/// `doctor --notify-test`: one notification through the nag's own path.
pub(crate) async fn notify_test(state: &State) -> Result<ResponseData, ErrorPayload> {
    let nagger = &state.nag;
    if let Some(why) = nagger.notifier.unavailable() {
        return Err(error_payload(ErrorKind::Unsupported, why.to_owned()));
    }
    let body = "Nag reminders can reach you on this machine";
    match nagger.notifier.show("ms-todo", body).await {
        Ok(()) => {
            nagger.set_failure(None);
            eprintln!("ms-todo daemon: nag: test notification shown");
            Ok(ResponseData::Ack)
        }
        Err(why) => {
            nagger.set_failure(Some(why.clone()));
            Err(error_payload(
                ErrorKind::Internal,
                format!("the test notification failed: {why}"),
            ))
        }
    }
}
