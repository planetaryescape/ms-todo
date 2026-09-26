//! Folders, and the changes `ChangeLists` makes to lists and folders.

use serde::{Deserialize, Serialize};

/// A folder of lists (docs/blueprint/05-custom-features.md#folders-list-groups).
/// It exists only as a name its lists carry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    pub name: String,
    /// Its lists' local IDs, in order.
    pub lists: Vec<String>,
    /// Open tasks in all its lists.
    pub open_count: u64,
}

/// A change to lists' folders or order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ListChange {
    /// Put `lists` (names or IDs) in the folder `folder`, made if it's
    /// new, or in no folder for `None`. Each goes last in its folder.
    MoveList {
        lists: Vec<String>,
        #[serde(default)]
        folder: Option<String>,
    },
    /// Put `list` next to `anchor`, a list in the same folder (or both in
    /// none).
    OrderList { list: String, anchor: Anchor },
    /// Rename the folder `folder` (its lists all move) to `name`, which no
    /// other folder has.
    RenameFolder { folder: String, name: String },
    /// Take every list out of the folder `folder`; no list is deleted.
    DeleteFolder { folder: String },
    /// Put the folder `folder` next to the folder in `anchor`.
    OrderFolder { folder: String, anchor: Anchor },
    /// A new list called `name`, in the folder `folder` when given.
    CreateList {
        name: String,
        #[serde(default)]
        folder: Option<String>,
    },
    /// Rename `list`. The default list and Flagged Emails can't be.
    RenameList { list: String, name: String },
    /// Delete `list` and every task in it. The default list and Flagged
    /// Emails can't be.
    DeleteList { list: String },
    #[serde(other)]
    Unknown,
}

/// Where a list or folder goes: just before or just after this one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    Before(String),
    After(String),
}
