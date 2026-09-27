//! Semantic search (rung 9c, D-062): finding tasks by meaning with a small
//! embedding model that runs in the daemon, so task text never leaves the
//! machine. Opt-in through `[search] semantic = true`, because turning it
//! on downloads the model (about 31 MB, once); nothing here touches the
//! network while it's off.
//!
//! The daemon loads the model when it starts with semantic search on, then
//! keeps a vector per task in the store (`index`), re-embedding only tasks
//! whose text changed, in the background after each change to the cache.
//! A search ranks those vectors against the query's (`query`).

pub(crate) mod config;
pub(crate) mod index;
pub(crate) mod model;
pub(crate) mod query;

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ms_todo_core::ErrorKind;
use ms_todo_protocol::{ErrorPayload, Event, ModelState, SemanticStatus};
use ms_todo_store::Store;
use tokio::sync::broadcast::error::{RecvError, TryRecvError};

use crate::events::Events;
use crate::handlers::{State, error_payload, store_error};
use config::Setting;
use model::{Embedder, MODEL_ID, Source};

/// Overrides where the model is read from, in debug builds: a directory
/// holding a ready model, used as it is with no download. Tests set it;
/// release builds ignore it, so they only ever run the pinned model.
pub(crate) const MODEL_DIR_ENV: &str = "MS_TODO_SEMANTIC_MODEL_DIR";

/// After a failed load, how long until the indexer tries again by itself
/// (a search tries at once).
const RETRY_AFTER: Duration = Duration::from_secs(5 * 60);

/// How long the indexer lets changes gather before a pass: a sync pass
/// changes one scope after another.
const SETTLE: Duration = Duration::from_secs(1);

pub(crate) struct Semantic {
    setting: Setting,
    config_file: String,
    source: Source,
    /// The model once loaded. Locked while loading, so one load runs at a
    /// time and a search that needs the model waits for it.
    model: tokio::sync::Mutex<Option<Arc<Embedder>>>,
    /// The last failure to load, since the last load that worked.
    problem: Mutex<Option<String>>,
}

impl Semantic {
    pub fn load(config_file: &Path, data_dir: &Path) -> Self {
        let setting = Setting::load(config_file);
        if let Setting::Invalid(why) = &setting {
            eprintln!("ms-todo daemon: semantic search is off: {why}");
        }
        let source = match std::env::var_os(MODEL_DIR_ENV) {
            Some(dir) if cfg!(debug_assertions) => Source::Local { dir: dir.into() },
            _ => Source::Pinned {
                dir: model::pinned_dir(data_dir),
            },
        };
        Self {
            setting,
            config_file: config_file.display().to_string(),
            source,
            model: tokio::sync::Mutex::new(None),
            problem: Mutex::new(None),
        }
    }

    pub fn enabled(&self) -> bool {
        self.setting == Setting::On
    }

    /// The model, loaded if it isn't yet: downloaded if needed, then the
    /// first indexing pass run before anyone gets it, so the first search
    /// sees every task. Waits for a load already running.
    pub async fn embedder(
        &self,
        store: &Store,
        events: &Events,
    ) -> Result<Arc<Embedder>, ErrorPayload> {
        self.check_enabled()?;
        let mut loaded = self.model.lock().await;
        if let Some(embedder) = loaded.as_ref() {
            return Ok(Arc::clone(embedder));
        }
        match model::load(&self.source).await {
            Ok(embedder) => {
                let embedder = Arc::new(embedder);
                eprintln!(
                    "ms-todo daemon: semantic search's model {MODEL_ID} is loaded from {}",
                    self.source.dir().display()
                );
                if let Err(why) = index::catch_up(store, &embedder).await {
                    eprintln!("ms-todo daemon: indexing tasks for semantic search failed: {why}");
                }
                *loaded = Some(Arc::clone(&embedder));
                self.set_problem(None);
                // A filter that was told the model wasn't ready can search now.
                events.index_changed();
                Ok(embedder)
            }
            Err(why) => {
                eprintln!("ms-todo daemon: semantic search's model can't be loaded: {why}");
                self.set_problem(Some(why.clone()));
                Err(error_payload(
                    ErrorKind::Network,
                    format!("semantic search's model isn't available: {why}"),
                ))
            }
        }
    }

    /// The model if it's loaded now. Never waits: while it's loading, or
    /// after a failure, says so instead.
    pub fn ready_embedder(&self) -> Result<Arc<Embedder>, ErrorPayload> {
        self.check_enabled()?;
        if let Ok(loaded) = self.model.try_lock()
            && let Some(embedder) = loaded.as_ref()
        {
            return Ok(Arc::clone(embedder));
        }
        let message = match self.problem() {
            Some(why) => format!("semantic search's model isn't available: {why}"),
            None => {
                "semantic search is getting ready (its model is loading); try again in a moment"
                    .into()
            }
        };
        Err(error_payload(ErrorKind::Network, message))
    }

    /// For `doctor`.
    pub async fn status(&self, store: &Store) -> Result<SemanticStatus, ErrorPayload> {
        let mut status = SemanticStatus {
            enabled: self.enabled(),
            model: MODEL_ID.into(),
            state: ModelState::Off,
            model_dir: self.source.dir().display().to_string(),
            download_bytes: model::download_bytes(),
            ..SemanticStatus::default()
        };
        match &self.setting {
            Setting::Off => return Ok(status),
            Setting::Invalid(why) => {
                status.problem = Some(why.clone());
                return Ok(status);
            }
            Setting::On => {}
        }
        let problem = self.problem();
        status.state = match self.model.try_lock() {
            Ok(loaded) if loaded.is_some() => ModelState::Ready,
            Ok(_) if problem.is_some() => ModelState::Failed,
            // Locked: loading now; neither: about to.
            _ => ModelState::Loading,
        };
        status.problem = problem;
        let (indexed, pending) = store
            .embedding_counts(MODEL_ID)
            .await
            .map_err(store_error)?;
        status.indexed = indexed.into();
        status.pending = pending.into();
        Ok(status)
    }

    /// Whether a semantic search can be asked for at all: when it's off,
    /// an error that says how to turn it on.
    pub fn check_enabled(&self) -> Result<(), ErrorPayload> {
        let message = match &self.setting {
            Setting::On => return Ok(()),
            Setting::Off => format!(
                "semantic search is off. It runs a small model on this computer, so task text \
                 never leaves it, but the model is a one-time download of about {} MB from \
                 Hugging Face. To turn it on, set `semantic = true` under [search] in {}, then \
                 restart the daemon with `ms-todo daemon stop`",
                model::download_bytes().div_ceil(1_000_000),
                self.config_file
            ),
            Setting::Invalid(why) => format!("semantic search is off: {why}"),
        };
        Err(error_payload(ErrorKind::InvalidInput, message))
    }

    fn problem(&self) -> Option<String> {
        self.problem.lock().ok().and_then(|problem| problem.clone())
    }

    fn set_problem(&self, problem: Option<String>) {
        if let Ok(mut slot) = self.problem.lock() {
            *slot = problem;
        }
    }
}

/// The indexer, for as long as the daemon runs with semantic search on:
/// load the model, embed what changed, then wait for the cache to change
/// again (or, after a failed load, for the time to retry).
pub(crate) async fn run(state: Arc<State>) {
    if !state.semantic.enabled() {
        return;
    }
    let mut events = state.events.subscribe();
    loop {
        let retry = match state.semantic.embedder(&state.store, &state.events).await {
            Ok(embedder) => {
                match index::catch_up(&state.store, &embedder).await {
                    Ok(0) => {}
                    Ok(count) => {
                        eprintln!("ms-todo daemon: embedded {count} task(s) for semantic search");
                        state.events.index_changed();
                    }
                    Err(why) => eprintln!(
                        "ms-todo daemon: indexing tasks for semantic search failed: {why}"
                    ),
                }
                None
            }
            Err(_) => Some(RETRY_AFTER),
        };
        let retry = async {
            match retry {
                Some(after) => tokio::time::sleep(after).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            changed = next_change(&mut events) => if !changed {
                return;
            },
            () = retry => {}
        }
        tokio::time::sleep(SETTLE).await;
        // What came meanwhile is in this pass too.
        while matches!(events.try_recv(), Ok(_) | Err(TryRecvError::Lagged(_))) {}
    }
}

/// Wait until the cache changes. False once the daemon is stopping.
async fn next_change(events: &mut tokio::sync::broadcast::Receiver<Event>) -> bool {
    loop {
        match events.recv().await {
            Ok(Event::EntityChanged(_) | Event::ResyncNeeded) | Err(RecvError::Lagged(_)) => {
                return true;
            }
            Ok(_) => {}
            Err(RecvError::Closed) => return false,
        }
    }
}
