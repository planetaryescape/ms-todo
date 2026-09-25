//! List suggestions (rung 6b, D-053): which list a task headed for the
//! inbox might belong in, from TypeSafe's Jev model. Opt-in through
//! `[suggest]` in config.toml, and suggest-only: nothing here files a
//! task. Every failure (no key, the network, a 4xx or 5xx, the 3-second
//! deadline) is no suggestion, logged once and shown by `doctor`; the
//! feature is never required.
//!
//! Only the daemon talks to TypeSafe (D-031). What it sends: the task's
//! title, and each candidate list's folder, name and up to five open task
//! titles. The API key stays in memory for the daemon's life.

pub(crate) mod api_key;
pub(crate) mod config;
pub(crate) mod criteria;
pub(crate) mod typesafe;

use std::path::Path;
use std::sync::{Arc, Mutex};

use ms_todo_core::ErrorKind;
use ms_todo_protocol::{ErrorPayload, ListSuggestion, SuggestStatus};
use ms_todo_store::Store;
use serde_json::Value;
use tokio::sync::OnceCell;

use crate::handlers::error_payload;
use api_key::ApiKey;
use config::{Config, Setting};
use criteria::{Candidates, ListInfo};
use typesafe::TypeSafe;

/// Overrides TypeSafe's URL in debug builds, so tests can point a real
/// daemon at a mock server. Release builds ignore it: it would let
/// whoever sets the environment receive the API key.
pub(crate) const TYPESAFE_URL_ENV: &str = "MS_TODO_TYPESAFE_URL";

const PROVIDER: &str = "typesafe";

pub(crate) struct Suggester {
    setting: Setting,
    config_file: String,
    client: TypeSafe,
    /// Resolved once per daemon, on the first suggestion: the key, or why
    /// there's none.
    key: OnceCell<Result<ApiKey, String>>,
    /// The options, for the sync generation they were built at.
    candidates: tokio::sync::Mutex<Option<(u64, Arc<Candidates>)>>,
    /// The last failure since the last suggestion that worked.
    problem: Mutex<Option<String>>,
}

impl Suggester {
    pub fn load(config_file: &Path) -> Self {
        let mut url = typesafe::DEFAULT_URL.to_owned();
        if cfg!(debug_assertions)
            && let Ok(custom) = std::env::var(TYPESAFE_URL_ENV)
        {
            url = custom;
        }
        let setting = Setting::load(config_file);
        if let Setting::Invalid(why) = &setting {
            eprintln!("ms-todo daemon: list suggestions are off: {why}");
        }
        Self {
            setting,
            config_file: config_file.display().to_string(),
            client: TypeSafe::new(url),
            key: OnceCell::new(),
            candidates: tokio::sync::Mutex::new(None),
            problem: Mutex::new(None),
        }
    }

    /// For `doctor`.
    pub fn status(&self) -> SuggestStatus {
        match &self.setting {
            Setting::Off => SuggestStatus::default(),
            Setting::Invalid(why) => SuggestStatus {
                problem: Some(why.clone()),
                ..SuggestStatus::default()
            },
            Setting::On(_) => SuggestStatus {
                enabled: true,
                provider: Some(PROVIDER.into()),
                problem: self.problem.lock().ok().and_then(|problem| problem.clone()),
            },
        }
    }

    /// The list a task titled `title` likely belongs in, if one is likely
    /// enough. An error only when suggestions are off or the title empty.
    pub async fn suggest(
        &self,
        store: &Store,
        generation: u64,
        title: &str,
    ) -> Result<Option<ListSuggestion>, ErrorPayload> {
        let config = match &self.setting {
            Setting::On(config) => config,
            Setting::Off => {
                return Err(error_payload(
                    ErrorKind::InvalidInput,
                    format!(
                        "list suggestions are off; to turn them on, set `enabled = true` under \
                         [suggest] in {}, then restart the daemon with `ms-todo daemon stop`",
                        self.config_file
                    ),
                ));
            }
            Setting::Invalid(why) => {
                return Err(error_payload(
                    ErrorKind::InvalidInput,
                    format!("list suggestions are off: {why}"),
                ));
            }
        };
        let title = title.trim();
        if title.is_empty() {
            return Err(error_payload(
                ErrorKind::InvalidInput,
                "a suggestion needs the task's title".into(),
            ));
        }
        match self.ask(config, store, generation, title).await {
            Ok(suggestion) => {
                self.set_problem(None);
                Ok(suggestion)
            }
            Err(why) => {
                self.set_problem(Some(why));
                Ok(None)
            }
        }
    }

    async fn ask(
        &self,
        config: &Config,
        store: &Store,
        generation: u64,
        title: &str,
    ) -> Result<Option<ListSuggestion>, String> {
        let key = self
            .key
            .get_or_init(|| {
                api_key::resolve(
                    std::env::var(api_key::API_KEY_ENV).ok(),
                    config.api_key_command.as_ref(),
                )
            })
            .await
            .as_ref()
            .map_err(Clone::clone)?;
        let candidates = self.candidates(config, store, generation).await?;
        // A choice of one is no choice: the model would be sure of it.
        if candidates.len() < 2 {
            return Ok(None);
        }
        let answer = self
            .client
            .choose(key, title, &candidates.criteria)
            .await
            .map_err(|error| error.to_string())?;
        let Some((list_id, list_name)) = candidates.lists.get(&answer.choice) else {
            return Err(format!(
                "TypeSafe chose {:?}, which isn't one of the lists",
                answer.choice
            ));
        };
        Ok(
            (answer.confidence >= config.min_confidence).then(|| ListSuggestion {
                list_id: list_id.clone(),
                list_name: list_name.clone(),
                confidence: answer.confidence,
            }),
        )
    }

    /// The options, built again only after a sync pass has finished.
    async fn candidates(
        &self,
        config: &Config,
        store: &Store,
        generation: u64,
    ) -> Result<Arc<Candidates>, String> {
        let mut cached = self.candidates.lock().await;
        if let Some((built_at, candidates)) = cached.as_ref()
            && *built_at == generation
        {
            return Ok(Arc::clone(candidates));
        }
        let lists = list_infos(store)
            .await
            .map_err(|error| ms_todo_core::message_with_causes(&error))?;
        let candidates = Arc::new(criteria::build(&lists, &config.exclude_folders));
        *cached = Some((generation, Arc::clone(&candidates)));
        Ok(candidates)
    }

    /// Record how the last suggestion went, logging a failure once rather
    /// than on every keystroke's pause.
    fn set_problem(&self, now: Option<String>) {
        let Ok(mut problem) = self.problem.lock() else {
            return;
        };
        if let Some(why) = &now
            && problem.as_ref() != Some(why)
        {
            eprintln!("ms-todo daemon: warning: no list suggestion: {why}");
        }
        *problem = now;
    }
}

/// Every live list with its open tasks' titles, newest first, from the
/// cache.
async fn list_infos(store: &Store) -> Result<Vec<ListInfo>, ms_todo_store::StoreError> {
    let mut infos = Vec::new();
    for list in store.lists().await? {
        let tasks = store.tasks_in_list(&list.local_id).await?;
        let open_titles = tasks
            .iter()
            .rev()
            .filter(|task| task.raw.get("status").and_then(Value::as_str) != Some("completed"))
            .take(criteria::EXAMPLES)
            .map(|task| task.title.clone())
            .collect();
        infos.push(ListInfo {
            folder: list.folder().map(str::to_owned),
            id: list.local_id,
            name: list.display_name,
            wellknown: list.wellknown_list_name,
            open_titles,
        });
    }
    Ok(infos)
}
