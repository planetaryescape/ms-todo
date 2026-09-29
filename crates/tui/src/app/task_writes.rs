//! Task writes a key makes at once, with no prompt: `x` completes or
//! reopens, and Enter in the undo picker deletes the completed copy
//! chosen.

use ms_todo_protocol::{Request, TaskChange};

use super::{App, Effect, Mode, Tag, Write, change};

impl App {
    /// `x`: complete the open tasks among the targets in one request, or
    /// reopen them when all are completed.
    pub(super) fn toggle_complete(&mut self) -> Vec<Effect> {
        let targets: Vec<_> = self
            .targets()
            .into_iter()
            .filter(|task| {
                !self
                    .status_pending
                    .values()
                    .any(|ids| ids.contains(&task.id))
            })
            .collect();
        if targets.is_empty() {
            return Vec::new();
        }
        let open: Vec<String> = targets
            .iter()
            .filter(|task| !task.completed)
            .map(|task| task.id.clone())
            .collect();
        let mut effect = if open.is_empty() {
            let ids = targets.iter().map(|task| task.id.clone()).collect();
            change(Write::Reopen, ids, TaskChange::Reopen)
        } else {
            change(Write::Complete, open, TaskChange::Complete)
        };
        self.status_requests += 1;
        if let (Tag::Write(write), Request::ChangeTasks { tasks, .. }) =
            (&effect.tag, &effect.request)
        {
            self.status_pending
                .insert(self.status_requests, tasks.clone());
            effect.tag = Tag::TaskStatus(self.status_requests, *write);
        }
        self.selection.clear();
        vec![effect]
    }

    pub(super) fn pick_copy(&mut self) -> Vec<Effect> {
        let Mode::Picker {
            target,
            candidates,
            index,
        } = std::mem::replace(&mut self.mode, Mode::Normal)
        else {
            return Vec::new();
        };
        let Some(copy) = candidates.get(index) else {
            return Vec::new();
        };
        vec![Effect {
            tag: Tag::Undo,
            request: Request::Undo {
                target: Some(target),
                copy: Some(copy.id.clone()),
                op_id: None,
                idempotency_key: None,
            },
        }]
    }
}
