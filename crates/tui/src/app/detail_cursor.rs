//! Keeping the detail cursor on the same step or link across refreshes.
//! [`DetailRow`] says where the cursor is drawn, by position; the anchor
//! says which step or link it's on, by ID, as task selection follows a
//! task's ID since 5a. After every update the position is found again
//! from the ID, so a step deleted above it on the phone doesn't slide the
//! cursor onto another. When the step itself is gone, or the link has
//! changed, the cursor moves to the nearest row and the next step or link
//! action does nothing but say so, rather than act on something the user
//! didn't pick.

use ms_todo_core::LOCAL_CHILD_PREFIX as LOCAL_PREFIX;

use super::steps::DetailRow;
use super::{App, Level, Task};

/// Which step or link the detail cursor is on, and whose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Anchor {
    /// The task's local ID.
    task: String,
    child: Child,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Child {
    /// A field or the Steps heading: nothing to follow.
    None,
    /// A step by ID, and its text, to find it again once a `local-…` ID
    /// becomes Graph's.
    Step { id: String, name: String },
    /// The link, by ID (`None` when there's no link yet).
    Link(Option<String>),
    /// An attachment by ID, and its name, to find it again once a
    /// `local-…` ID becomes Graph's.
    Attachment { id: String, name: String },
}

impl App {
    /// Put the cursor on `row` of the selected task, following whatever
    /// is there now.
    pub(super) fn set_detail_row(&mut self, row: DetailRow) {
        self.detail_row = row;
        self.detail_stale = false;
        self.detail_anchor = self.selected().map(|task| anchor(task, row));
    }

    /// After anything changed: find the anchored step or link again.
    pub(super) fn follow_detail_row(&mut self) {
        let Some(task) = self.selected() else {
            return;
        };
        let same_task = self
            .detail_anchor
            .as_ref()
            .is_some_and(|anchor| anchor.task == task.id);
        if !same_task {
            // Another task: its row at the same position.
            let row = self.detail_row.on(task);
            self.set_detail_row(row);
            return;
        }
        let Some(anchor) = self.detail_anchor.clone() else {
            return;
        };
        let (row, stale) = match &anchor.child {
            Child::None => return,
            Child::Step { id, name } => match find_step(task, id, name, self.detail_row) {
                Some(at) => (DetailRow::Step(at), false),
                // Gone: the nearest step, or the heading.
                None => (self.detail_row.on(task), true),
            },
            Child::Link(id) => (DetailRow::Link, *id != task.link_id),
            Child::Attachment { id, name } => {
                let attachments: Vec<(&str, &str)> = task
                    .attachments
                    .iter()
                    .map(|attachment| (attachment.id.as_str(), attachment.name.as_str()))
                    .collect();
                let was = match self.detail_row {
                    DetailRow::Attachment(at) => Some(at),
                    _ => None,
                };
                match find_child(&attachments, id, name, was) {
                    Some(at) => (DetailRow::Attachment(at), false),
                    None => (self.detail_row.on(task), true),
                }
            }
        };
        let anchor = anchor_for(task, row);
        self.detail_row = row;
        self.detail_anchor = Some(anchor);
        self.detail_stale |= stale;
    }

    /// Whether a step or link action must wait: what the cursor was on
    /// has changed since. Says so, once, and clears it.
    pub(super) fn refuse_if_stale(&mut self) -> bool {
        if !std::mem::take(&mut self.detail_stale) {
            return false;
        }
        let what = match self.detail_row {
            DetailRow::Link => "The link changed",
            DetailRow::Attachments | DetailRow::Attachment(_) => "That attachment changed",
            _ => "That step changed",
        };
        self.show(Level::Info, &format!("{what}; nothing was done"));
        true
    }
}

fn anchor(task: &Task, row: DetailRow) -> Anchor {
    anchor_for(task, row.on(task))
}

fn anchor_for(task: &Task, row: DetailRow) -> Anchor {
    let child = match row {
        DetailRow::Step(at) => task.steps.get(at).map_or(Child::None, |step| Child::Step {
            id: step.id.clone(),
            name: step.name.clone(),
        }),
        DetailRow::Link => Child::Link(task.link_id.clone()),
        DetailRow::Attachment(at) => {
            task.attachments
                .get(at)
                .map_or(Child::None, |attachment| Child::Attachment {
                    id: attachment.id.clone(),
                    name: attachment.name.clone(),
                })
        }
        DetailRow::Field(_) | DetailRow::Steps | DetailRow::Attachments => Child::None,
    };
    Anchor {
        task: task.id.clone(),
        child,
    }
}

/// Where the step `id` is now.
fn find_step(task: &Task, id: &str, name: &str, was: DetailRow) -> Option<usize> {
    let steps: Vec<(&str, &str)> = task
        .steps
        .iter()
        .map(|step| (step.id.as_str(), step.name.as_str()))
        .collect();
    let was = match was {
        DetailRow::Step(at) => Some(at),
        _ => None,
    };
    find_child(&steps, id, name, was)
}

/// Where the child `id` is now among `children` (`(id, name)`, in order).
/// A step or attachment ms-todo added has a `local-…` ID until Microsoft
/// To Do answers, then Graph's: that's the same one if it has the same
/// name, where it was (`was`) or the only one with that name.
fn find_child(
    children: &[(&str, &str)],
    id: &str,
    name: &str,
    was: Option<usize>,
) -> Option<usize> {
    if let Some(at) = children.iter().position(|(child, _)| *child == id) {
        return Some(at);
    }
    if !id.starts_with(LOCAL_PREFIX) {
        return None;
    }
    let named = |(child, child_name): &(&str, &str)| {
        *child_name == name && !child.starts_with(LOCAL_PREFIX)
    };
    if let Some(at) = was
        && children.get(at).is_some_and(named)
    {
        return Some(at);
    }
    let mut matching = children
        .iter()
        .enumerate()
        .filter(|(_, child)| named(child));
    match (matching.next(), matching.next()) {
        (Some((at, _)), None) => Some(at),
        _ => None,
    }
}
