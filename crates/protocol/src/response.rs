//! The daemon's answer to a request: [`Response`], the data an answer
//! carries, and the error a failed one carries.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    Applied, AppliedContext, Contexts, DaemonStatus, DoctorReport, Entity, Folder, MyDay, OutboxOp,
    Plan, Seed, SyncInfo, SyncReport,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    Ok {
        data: ResponseData,
    },
    Error {
        error: ErrorPayload,
    },
    #[serde(other)]
    Unknown,
}

impl From<Result<ResponseData, ErrorPayload>> for Response {
    fn from(result: Result<ResponseData, ErrorPayload>) -> Self {
        match result {
            Ok(data) => Self::Ok { data },
            Err(error) => Self::Error { error },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResponseData {
    Status(DaemonStatus),
    Lists {
        items: Vec<Entity>,
        sync: SyncInfo,
    },
    Folders {
        items: Vec<Folder>,
        sync: SyncInfo,
    },
    Tasks {
        items: Vec<Entity>,
        sync: SyncInfo,
        /// For `ListTasks`: how many deferred and Someday tasks matched
        /// but were left out.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        deferred_hidden: Option<u64>,
        /// The context that narrowed them, if one did.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<AppliedContext>,
    },
    /// Tasks a search matched, best first: each task entity with `list`,
    /// its list's name, and `snippet`, the passage that matched on one
    /// line with each match between `**`s. A semantic search's have
    /// `score` instead, their cosine similarity to the query, and
    /// `semantic` says how complete the index was.
    SearchResults {
        items: Vec<Entity>,
        sync: SyncInfo,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        semantic: Option<SemanticIndex>,
        /// The context that narrowed them, if one did.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<AppliedContext>,
    },
    Sync(SyncReport),
    /// Boxed, as `Seed` is: every part of the daemon reports in it.
    Doctor(Box<DoctorReport>),
    Raw {
        body: Value,
    },
    /// What a mutation would do, from a dry run.
    Plan(Plan),
    /// What a mutation did.
    Applied(Applied),
    /// Outbox operations, newest first.
    Outbox {
        items: Vec<OutboxOp>,
    },
    /// The operation `outbox retry` or `outbox discard` acted on, as it is
    /// now (as it was, for a discard).
    OutboxOp(OutboxOp),
    Bearer {
        access_token: String,
        /// Unix seconds.
        expires_at: i64,
    },
    /// Boxed: by far the largest answer, and one per screen.
    Seed(Box<Seed>),
    /// The answer to `SuggestList`: `None` when no list is likely enough.
    ListSuggestion {
        suggestion: Option<ListSuggestion>,
    },
    MyDay(MyDay),
    /// The files `DownloadAttachments` wrote, in order.
    Downloaded {
        /// The task's local ID.
        task_id: String,
        files: Vec<DownloadedFile>,
    },
    /// Outlook categories as Graph has them (`id`, `displayName`,
    /// `color`), or open extensions (each with `extensionName`).
    Categories {
        items: Vec<Entity>,
    },
    Extensions {
        items: Vec<Entity>,
    },
    Contexts(Contexts),
    Ack,
    #[serde(other)]
    Unknown,
}

/// An attachment written to disk.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadedFile {
    /// The attachment's ID.
    pub id: String,
    /// Its name, as Microsoft To Do has it.
    pub name: String,
    /// Where it was written: an absolute path.
    pub path: String,
    /// How many bytes were written.
    pub bytes: u64,
    /// The bytes' sha256, as hex.
    pub sha256: String,
}

/// A list a task might belong in, and how sure the provider is.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ListSuggestion {
    /// The list's local ID.
    pub list_id: String,
    pub list_name: String,
    /// From 0 to 1; at least the configured `min_confidence`.
    pub confidence: f64,
}

/// A list or task by its local ID and display name (a task's title).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub name: String,
    /// For a task: when Graph created it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    /// For a task: its list's local ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list_id: Option<String>,
}

/// A failed request. `kind` is an `ms_todo_core::ErrorKind` string; a kind
/// the client doesn't know is treated as `internal`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorPayload {
    pub kind: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// The lists or tasks an ambiguous name matched.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<Candidate>,
    /// The mutation's `op_id`, on a failed or `outcome_unknown` mutation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op_id: Option<String>,
    /// Tasks a multi-task mutation changed before it failed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub applied: Vec<String>,
    /// For an undo that needs `copy`: the `op_id` it would undo, so the
    /// retry with the chosen copy undoes that one and no other.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo_target: Option<String>,
}

/// How complete the semantic index was for a search (D-062).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticIndex {
    /// The embedding model, as `name@revision`.
    pub model: String,
    /// Tasks in the search's scope not yet embedded for their current
    /// text: they're missing from the results, or ranked by their old text.
    pub pending: u32,
}
