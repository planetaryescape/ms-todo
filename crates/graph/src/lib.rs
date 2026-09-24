//! Microsoft Graph provider. In F1 it only signs in: device code, the token
//! store and the compare-and-swap refresh (docs/blueprint/03-graph-provider.md).
//! The HTTP client, retries and endpoints arrive in rung 1.

#[cfg(not(unix))]
compile_error!("ms-todo supports macOS and Linux only; the token store relies on Unix file modes");

pub mod auth;
