//! Microsoft Graph provider: sign-in (`auth`, the only module the CLI may
//! use) and, for the daemon, the HTTP client with retry, pagination and
//! typed errors (docs/blueprint/03-graph-provider.md).

#[cfg(not(unix))]
compile_error!("ms-todo supports macOS and Linux only; the token store relies on Unix file modes");

mod api_error;
mod attachments;
pub mod auth;
mod batch;
mod children;
mod client;
mod error;
pub mod private_file;
pub mod retry;

pub use api_error::ApiError;
pub use attachments::MAX_ATTACHMENT_BYTES;
pub use client::{Delta, Entity, GraphClient};
pub use error::GraphError;
/// The HTTP method of a `raw` write.
pub use reqwest::Method;
