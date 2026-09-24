//! Types every ms-todo crate shares. This crate must stay free of I/O
//! dependencies (tokio, reqwest, sqlx, file locks); `tests/workspace_boundaries.rs`
//! enforces that.

mod error;
mod paths;

pub use error::{ErrorKind, message_with_causes};
pub use paths::{
    APP_NAME, CONFIG_DIR_ENV, INSTANCE_ENV, Instance, InvalidInstanceName, Paths, PathsError,
    config_dir_from,
};
