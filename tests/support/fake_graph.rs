//! The Graph double lives in `crates/fake-graph`, shared with the demo
//! harness; this points it at a test's [`Env`].

#[allow(unused_imports, reason = "each test binary uses a different subset")]
pub use ms_todo_fake_graph::graph::*;

use super::Env;

impl ms_todo_fake_graph::GraphUser for Env {
    fn use_graph(&mut self, url: String) {
        self.graph_url = Some(url);
    }

    fn sign_in(&self) {
        Env::sign_in(self);
    }
}
