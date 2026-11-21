//! Test-only gateway.
//!
//! `cargo test` must stay offline and deterministic: no API keys, no
//! network, no flaky assertions. `MockGateway` is the injection point for
//! every test that exercises an `ai.*` node — it records the requests it
//! received and answers with a caller-supplied pure transform, and
//! [`MockGateway::with_providers`] seeds the global provider catalog the
//! `ai.completion` node resolves a model against.

use std::collections::VecDeque;
use std::sync::Mutex;

use super::{
    AiError, AiGateway, AiRequest, BoxFuture, ChatMessage, ChatTurn, ProviderInfo, ToolSpec,
};
use crate::app_tools::AppTool;

/// In-memory gateway backed by a `fn(&str) -> String` transform.
pub struct MockGateway {
    transform: fn(&str) -> String,
    providers: Vec<ProviderInfo>,
    tools: Vec<AppTool>,
    /// Pre-scripted answers for tool-aware turns, consumed in order. When
    /// empty, `complete_turn` falls back to a plain completion.
    turns: Mutex<VecDeque<ChatTurn>>,
    calls: Mutex<Vec<AiRequest>>,
}

impl MockGateway {
    pub fn new(transform: fn(&str) -> String) -> Self {
        Self {
            transform,
            providers: Vec::new(),
            tools: Vec::new(),
            turns: Mutex::new(VecDeque::new()),
            calls: Mutex::new(Vec::new()),
        }
    }

    /// Pretend the app has these app-level tools configured.
    pub fn with_tools(mut self, tools: Vec<AppTool>) -> Self {
        self.tools = tools;
        self
    }

    /// Script the answers returned by `complete_turn`, in call order.
    pub fn with_turns(mut self, turns: Vec<ChatTurn>) -> Self {
        self.turns = Mutex::new(turns.into());
        self
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

    fn tools(&self) -> Result<Vec<AppTool>, AiError> {
        Ok(self.tools.clone())
    }

    fn complete_turn<'a>(
        &'a self,
        req: &'a AiRequest,
        messages: &'a [ChatMessage],
        _tools: &'a [ToolSpec],
    ) -> BoxFuture<'a, Result<ChatTurn, AiError>> {
        Box::pin(async move {
            if let Some(scripted) = self
                .turns
                .lock()
                .expect("mock gateway mutex")
                .pop_front()
            {
                self.calls
                    .lock()
                    .expect("mock gateway mutex")
                    .push(req.clone());
                return Ok(scripted);
            }
            // No script: behave like a plain completion of the latest user
            // message, so a test that only cares about the prompt still
            // gets a deterministic answer.
            let prompt = messages
                .iter()
                .rev()
                .find_map(|m| match m {
                    ChatMessage::User(text) => Some(text.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| req.prompt.clone());
            let mut flattened = req.clone();
            flattened.prompt = prompt;
            let text = (self.transform)(&flattened.prompt);
            self.calls
                .lock()
                .expect("mock gateway mutex")
                .push(flattened);
            Ok(ChatTurn {
                text,
                tool_calls: Vec::new(),
            })
        })
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
