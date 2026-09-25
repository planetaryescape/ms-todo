//! Types every ms-todo crate shares. This crate must stay free of I/O
//! dependencies (tokio, reqwest, sqlx, file locks); `tests/workspace_boundaries.rs`
//! enforces that.

mod dates;
mod display;
mod error;
pub mod links;
mod paths;

pub use dates::{
    DATE_FORMAT, REMINDER_FORMAT, completion_date, completion_heading, day_heading,
    local_date_time, local_due_date, parse_graph_date_time,
};
pub use display::{CONTROL_PLACEHOLDER, display_safe, one_line_safe};
pub use error::{ErrorKind, message_with_causes};
pub use paths::{
    APP_NAME, CONFIG_DIR_ENV, INSTANCE_ENV, Instance, InvalidInstanceName, Paths, PathsError,
    config_dir_from,
};

/// How the ID of a step or link ms-todo made starts until Microsoft To Do
/// answers its create with Graph's ID (D-055). Graph's own child IDs are
/// bare UUIDs.
pub const LOCAL_CHILD_PREFIX: &str = "local-";
