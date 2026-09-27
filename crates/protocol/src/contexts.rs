//! Contexts (rung 9d): named sets of lists from `[contexts.<name>]` in
//! config.toml, one of them active in the daemon, which narrows the
//! everyday reads to its lists (docs/blueprint/05-custom-features.md#contexts).

use serde::{Deserialize, Serialize};

use crate::Candidate;

/// Which context a request is read in, when not the active one:
/// `--context <name|none>`, carried by `Request::InContext`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "use", rename_all = "snake_case")]
pub enum ContextChoice {
    /// No context: everything.
    None,
    /// The context of this name.
    Named { name: String },
    #[serde(other)]
    Unknown,
}

/// The context a read was narrowed by, as its answer says: in JSON's
/// envelope, and as `context: work (7 lists)` under a table.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedContext {
    pub name: String,
    /// How many lists it covers.
    pub lists: u64,
    /// The name of the list a task goes to when none is named: the
    /// context's `default_list`, when it names one of its lists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_list: Option<String>,
}

/// A context as config.toml defines it and the cache resolves it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextInfo {
    pub name: String,
    /// Whether it's the daemon's active context.
    pub active: bool,
    /// The folders it takes every list of, as configured.
    pub folders: Vec<String>,
    /// The lists it names, as configured.
    pub lists: Vec<String>,
    /// Where a task goes when none is named, as configured.
    #[serde(default)]
    pub default_list: Option<String>,
    /// The lists it covers now, in the sidebar's order: its folders'
    /// lists and its named lists together.
    pub resolved: Vec<Candidate>,
    /// What doesn't resolve, such as a folder or list no longer there.
    #[serde(default)]
    pub problems: Vec<String>,
}

/// The answer to `Contexts` and `SetContext`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contexts {
    /// The active context's name; `None` shows everything.
    pub active: Option<String>,
    /// Every context in config.toml, by name.
    pub items: Vec<ContextInfo>,
    /// What's wrong beyond one context: config.toml unreadable, or an
    /// active context it no longer defines.
    #[serde(default)]
    pub problems: Vec<String>,
}
