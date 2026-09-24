//! The tasks this daemon has read or written, by Graph ID: which list each
//! is in and its last-seen state. Graph has no task path without the list
//! (`/me/todo/lists/{l}/tasks/{t}`), and `If-Match` needs the etag from our
//! last read or write (S6), so `tasks complete ID` looks here first.
//!
//! Rung 2 only: it lives in memory, so a restarted daemon finds tasks again
//! by asking each list. Rung 3a's store replaces it.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use ms_todo_protocol::Entity;

use crate::list_resolution::field;

/// A task and its list, as last read or written, with the etag.
#[derive(Clone, Debug)]
pub(crate) struct Target {
    pub list_id: String,
    pub task: Entity,
}

impl Target {
    pub fn id(&self) -> &str {
        field(&self.task, "id").unwrap_or_default()
    }

    pub fn title(&self) -> &str {
        field(&self.task, "title").unwrap_or_default()
    }
}

#[derive(Default)]
pub(crate) struct KnownTasks {
    tasks: Mutex<HashMap<String, Target>>,
}

impl KnownTasks {
    pub fn get(&self, task_id: &str) -> Option<Target> {
        self.lock().get(task_id).cloned()
    }

    pub fn remember(&self, list_id: &str, task: &Entity) {
        self.remember_all(list_id, std::slice::from_ref(task));
    }

    pub fn remember_all(&self, list_id: &str, tasks: &[Entity]) {
        let mut known = self.lock();
        for task in tasks {
            if let Some(id) = field(task, "id") {
                known.insert(
                    id.to_owned(),
                    Target {
                        list_id: list_id.to_owned(),
                        task: task.clone(),
                    },
                );
            }
        }
    }

    pub fn forget(&self, task_id: &str) {
        self.lock().remove(task_id);
    }

    // No code panics while holding the lock, and the map stays usable even
    // if one did, so a poisoned lock is still taken.
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Target>> {
        self.tasks.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
