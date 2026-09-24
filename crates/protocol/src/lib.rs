//! The IPC protocol between ms-todo's clients and its daemon
//! (docs/blueprint/01-architecture.md#transport): length-delimited JSON over
//! a Unix socket, each frame a [`Message`] envelope `{ id, payload }`.
//!
//! Compatibility rules, so an older client and a newer daemon (or the other
//! way round) fail clearly instead of mis-reading each other:
//!
//! - Every tagged enum has an `Unknown` variant that a tag this build doesn't
//!   know decodes to.
//! - Fields added after rung 1 carry `#[serde(default)]`.
//! - A change that can't follow those rules bumps [`PROTOCOL_VERSION`].
//!   Clients check it through `Status` before anything else, and restart
//!   the daemon when it differs.

mod codec;

pub use codec::{Codec, FrameTooLarge, MAX_FRAME_BYTES};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Bumped on any change an older peer can't read.
pub const PROTOCOL_VERSION: u32 = 1;

/// A JSON object from Graph, every field kept. Its `id` is the Graph ID in
/// rungs 1 and 2 (docs/blueprint/07-cli.md#output-contract).
pub type Entity = Map<String, Value>;

/// One frame. A client picks `id`, and the daemon echoes it on the response.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub id: u64,
    pub payload: Payload,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Payload {
    Request(Request),
    Response(Response),
    Event(Event),
    #[serde(other)]
    Unknown,
}

/// What a client can ask the daemon for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    /// Readiness and version check; answers even while signed out.
    Status,
    ListLists,
    /// Tasks of the list named or identified by `list`, or of the default
    /// list ("Tasks") when `None`.
    ListTasks {
        #[serde(default)]
        list: Option<String>,
    },
    /// An authenticated GET of a path under the Graph v1.0 root.
    RawGet {
        path: String,
    },
    /// A valid access token, for `auth bearer --reveal-secret`.
    Bearer,
    /// Stop the daemon. It answers `Ack`, then exits.
    Shutdown,
    #[serde(other)]
    Unknown,
}

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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResponseData {
    Status(DaemonStatus),
    Lists {
        items: Vec<Entity>,
    },
    Tasks {
        items: Vec<Entity>,
    },
    Raw {
        body: Value,
    },
    Bearer {
        access_token: String,
        /// Unix seconds.
        expires_at: i64,
    },
    Ack,
    #[serde(other)]
    Unknown,
}

/// What `Status` reports. Ready means this answers with a compatible
/// `protocol_version` (vault: `Daemon Readiness Is Not Process Liveness`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub protocol_version: u32,
    /// The daemon's package version.
    pub version: String,
    pub pid: u32,
    pub instance: String,
    /// Unix seconds.
    pub started_at: i64,
    /// Whether a credential is stored. It may still be revoked.
    #[serde(default)]
    pub signed_in: bool,
}

/// A list by its ID and display name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListRef {
    pub id: String,
    pub name: String,
}

/// A failed request. `kind` is an `ms_todo_core::ErrorKind` string; a kind
/// the client doesn't know is treated as `internal`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorPayload {
    pub kind: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// The lists an ambiguous `--list` name matched.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<ListRef>,
}

/// Pushed by the daemon. Rung 1 sends none; the type exists so a client
/// built now skips the events later rungs add.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    #[serde(other)]
    Unknown,
}
