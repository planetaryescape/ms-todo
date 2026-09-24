//! Types every ms-todo crate shares. This crate must stay free of I/O
//! dependencies (tokio, reqwest, sqlx, file locks); `tests/workspace_boundaries.rs`
//! enforces that.

mod error;
mod paths;

pub use error::ErrorKind;
pub use paths::{APP_NAME, INSTANCE_ENV, Instance, InvalidInstanceName, Paths, PathsError};
