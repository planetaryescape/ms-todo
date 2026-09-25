//! Outlook categories and open extensions (rung 8e): what lives beside
//! the lists and tasks rather than in them, read and written straight
//! through the daemon to Graph (D-058), not cached.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// A change to the user's Outlook categories (`/me/outlook/masterCategories`).
/// Graph can't rename one (S7), so there's no rename.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum CategoryChange {
    /// A new category. Names are unique ignoring case (S7).
    Create {
        name: String,
        /// `preset0`–`preset24` or `none`; `None` is Graph's default.
        #[serde(default)]
        color: Option<String>,
    },
    /// A category's colour. `category` is its name (ignoring case) or ID.
    Recolor { category: String, color: String },
    /// Delete a category. Tasks keep the name as a label.
    Delete { category: String },
    #[serde(other)]
    Unknown,
}

/// What an open extension belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerKind {
    List,
    Task,
}

/// A list or task, as the extension commands name it: a list by its name
/// or ID; a task by its ID, or its exact title in `list`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionOwner {
    pub kind: OwnerKind,
    pub id: String,
    #[serde(default)]
    pub list: Option<String>,
}

/// A change to one open extension.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ExtensionChange {
    /// Make the extension `name` hold exactly `data` (made if missing).
    Set {
        name: String,
        data: Map<String, Value>,
    },
    Delete {
        name: String,
    },
    #[serde(other)]
    Unknown,
}
