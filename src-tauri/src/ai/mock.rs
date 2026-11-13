//! Test-only gateway.
//!
//! `cargo test` must stay offline and deterministic: no API keys, no
//! network, no flaky assertions. `MockGateway` is the injection point for
//! every test that exercises an `ai.*` node — it records the requests it
//! received and answers with a caller-supplied pure transform, and
//! [`MockGateway::with_providers`] seeds the global provider catalog the
//! `ai.completion` node resolves a model against.

use std::sync::Mutex;

use super::{AiError, AiGateway, AiRequest, BoxFuture, ProviderInfo};

/// In-memory gateway backed by a `fn(&str) -> String` transform.
pub struct MockGateway {
    transform: fn(&str) -> String,
    providers: Vec<ProviderInfo>,
    calls: Mutex<Vec<AiRequest>>,
}

impl MockGateway {
    pub fn new(transform: fn(&str) -> String) -> Self {
        Self {
            transform,
            providers: Vec::new(),
            calls: Mutex::new(Vec::new()),
        }
    }

    /// Pretend the app has these providers configured globally, so a test
    /// can exercise the node's provider/model resolution offline.
    pub fn with_providers(mut self, providers: Vec<ProviderInfo>) -> Self {
        self.providers = providers;
        self
    }

    /// Snapshot of every request this gateway answered, in call order.
    pub fn calls(&self) -> Vec<AiRequest> {
        self.calls.lock().expect("mock gateway mutex").clone()
    }
}

impl AiGateway for MockGateway {
    fn providers(&self) -> Result<Vec<ProviderInfo>, AiError> {
        Ok(self.providers.clone())
    }

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
