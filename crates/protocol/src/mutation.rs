//! A mutation's typed plan, from a dry run, and what the real run did.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Candidate, Entity};

/// What a mutation did, for tasks or, since protocol 6, lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskAction {
    Add,
    Complete,
    Reopen,
    Edit,
    Delete,
    Undo,
    /// Tasks to another list.
    Move,
    /// Lists into a folder, or out of any.
    MoveList,
    /// A list before or after another in its folder.
    OrderList,
    RenameFolder,
    /// A folder emptied: its lists stay, in no folder.
    DeleteFolder,
    /// A folder before or after another.
    OrderFolder,
    /// Tasks into today's My Day.
    MyDayAdd,
    /// Tasks out of My Day.
    MyDayRemove,
    /// An earlier day's My Day emptied.
    MyDayRollover,
    StepAdd,
    StepEdit,
    StepCheck,
    StepUncheck,
    StepDelete,
    LinkAdd,
    LinkEdit,
    LinkDelete,
    AttachmentAdd,
    AttachmentDelete,
    CreateList,
    RenameList,
    DeleteList,
    CategoryCreate,
    CategoryRecolor,
    CategoryDelete,
    ExtensionSet,
    ExtensionDelete,
    /// Two tasks linked to each other (D-067).
    Relate,
    /// The link between two tasks taken away.
    Unrelate,
    /// Every task of one list moved to another, and the emptied list
    /// optionally deleted.
    MergeList,
    #[serde(other)]
    Unknown,
}

/// A list a plan changes, and the extension fields it gets (a null
/// removes one).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedList {
    pub id: String,
    pub name: String,
    pub changes: Value,
}

/// A mutation's typed plan: the verb and its resolved targets. A dry run
/// renders it and the real run applies it, so the preview is what runs
/// (docs/blueprint/07-cli.md#global-flags).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub action: TaskAction,
    /// The list a task is added to, or tasks are moved to. Empty for other
    /// actions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list: Option<Candidate>,
    /// The tasks changed, in order. Empty for `add`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<PlannedTask>,
    /// For a change to lists: each list changed, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lists: Vec<PlannedList>,
    /// The Graph fields each target gets (the POST body for `add`, without
    /// the `opId` extension a real run adds). Null for `delete`.
    #[serde(default)]
    pub changes: Value,
}

/// A task a plan changes, by local IDs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedTask {
    pub id: String,
    pub title: String,
    pub list_id: String,
}

/// What a mutation did.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Applied {
    /// The create's `opId`, or a fresh UUID for other actions.
    pub op_id: String,
    pub action: TaskAction,
    /// Each task as Graph returned it after the change, in the entity shape
    /// (local `id`, `graph_id`). For `delete`, as it was last read. For a
    /// change to lists, each list as it is now.
    pub items: Vec<Entity>,
    /// The local ID of the list each of `items` is in, in the same order.
    #[serde(default)]
    pub list_ids: Vec<String>,
    /// Recurring tasks this completed: Graph kept the task, moved its due
    /// date on, and made a completed copy with a new ID (S12).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rolled: Vec<Rolled>,
    /// For an undo: the `op_id` it undoes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undoes: Option<String>,
    /// For an undo of a change to several tasks: the tasks it left alone,
    /// because a field the change set has changed since.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refused: Vec<Refused>,
}

/// A task an undo left alone, and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refused {
    /// The task's local ID.
    pub id: String,
    pub title: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rolled {
    pub id: String,
    /// `YYYY-MM-DD`, local.
    pub next_due: String,
}
