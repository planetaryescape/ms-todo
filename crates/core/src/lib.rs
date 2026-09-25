//! Types every ms-todo crate shares. This crate must stay free of I/O
//! dependencies (tokio, reqwest, sqlx, file locks); `tests/workspace_boundaries.rs`
//! enforces that.

mod dates;
mod display;
mod error;
mod paths;

pub use dates::{
    DATE_FORMAT, REMINDER_FORMAT, local_date_time, local_due_date, parse_graph_date_time,
};
pub use display::{CONTROL_PLACEHOLDER, display_safe, one_line_safe};
pub use error::{ErrorKind, message_with_causes};
pub use paths::{
    APP_NAME, CONFIG_DIR_ENV, INSTANCE_ENV, Instance, InvalidInstanceName, Paths, PathsError,
    config_dir_from,
};
