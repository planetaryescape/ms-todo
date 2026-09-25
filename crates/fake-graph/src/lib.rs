//! A Microsoft Graph double, shared by the integration tests (through
//! `tests/support`) and the demo harness (`demo/fake-graph`), so both
//! exercise the same behaviour the spikes observed.

pub mod graph;
pub mod moves;

pub use graph::*;
pub use moves::*;

/// What [`FakeGraph::start`] needs from whatever runs ms-todo against it:
/// somewhere to point the daemon, and a signed-in credential.
pub trait GraphUser {
    /// Send Graph requests to `url` (the base, ending `/v1.0`).
    fn use_graph(&mut self, url: String);
    fn sign_in(&self);
}
