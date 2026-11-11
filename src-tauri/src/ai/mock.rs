//! Test-only gateway.
//!
//! `cargo test` must stay offline and deterministic: no API keys, no
//! network, no flaky assertions. `MockGateway` is the injection point for
//! every test that exercises an `ai.*` node — it records the requests it
//! received and answers with a caller-supplied pure transform.

use std::sync::Mutex;

use super::{AiError, AiGateway, AiRequest, BoxFuture};

/// In-memory gateway backed by a `fn(&str) -> String` transform.
pub struct MockGateway {
    transform: fn(&str) -> String,
    calls: Mutex<Vec<AiRequest>>,
}

impl MockGateway {
    pub fn new(transform: fn(&str) -> String) -> Self {
        Self {
            transform,
            calls: Mutex::new(Vec::new()),
        }
    }

    /// Snapshot of every request this gateway answered, in call order.
    pub fn calls(&self) -> Vec<AiRequest> {
        self.calls.lock().expect("mock gateway mutex").clone()
    }
}

impl AiGateway for MockGateway {
    fn complete<'a>(&'a self, req: &'a AiRequest) -> BoxFuture<'a, Result<String, AiError>> {
        Box::pin(async move {
            self.calls
                .lock()
                .expect("mock gateway mutex")
                .push(req.clone());
            Ok((self.transform)(&req.prompt))
        })
    }
}
