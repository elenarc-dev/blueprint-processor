//! AI provider layer.
//!
//! Three pieces, each with one reason to exist:
//!
//! 1. [`secrets`] — where an API key lives. **Never in a blueprint file.**
//!    Blueprints are saved to disk, diffed and shared; a key in
//!    `params` would leak the moment the user shares a graph.
//! 2. [`providers`] — translates an [`AiRequest`] into a provider call.
//!    The only module that knows about `rig-core`, so the pre-1.0 API
//!    churn of that crate stays contained.
//! 3. [`AiGateway`] — the seam the executor sees. [`RigGateway`] is the
//!    real one; `mock::MockGateway` lets `cargo test` run the complete
//!    pipeline offline.
//!
//! `AiGateway` is object-safe on purpose: the async node registry stores
//! executors as plain fn pointers, so the gateway has to travel as an
//! `Arc<dyn AiGateway>` inside [`AiEnv`]. `complete` returns a boxed
//! future instead of using `async fn` in the trait to keep that
//! dyn-compatibility.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use thiserror::Error;

pub mod providers;
pub mod secrets;

#[cfg(test)]
pub mod mock;

pub use secrets::{SecretStatus, SecretStore};

/// Boxed future carrying the gateway's result. `Send` because the runner
/// is awaited from a Tauri async command.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Providers the `ai.completion` node can talk to.
///
/// `OpenAi` is the OpenAI-compatible wire (key + optional `base_url`),
/// which also covers Groq, OpenRouter and local Ollama / LM Studio
/// endpoints. `Anthropic` is the Messages wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiProviderKind {
    OpenAi,
    Anthropic,
}

impl AiProviderKind {
    /// Canonical provider names a blueprint may carry. Order matters: it
    /// is the order the settings dialog lists them in.
    pub const ALL: [&'static str; 2] = ["openai", "anthropic"];

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "openai" | "openai-compatible" | "openai_compatible" => Some(Self::OpenAi),
            "anthropic" => Some(Self::Anthropic),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
        }
    }

    /// Model used when the node leaves `model` blank. Deliberately the
    /// cheap/fast model of each provider so a stray Run is not a
    /// surprise bill.
    pub fn default_model(self) -> &'static str {
        match self {
            Self::OpenAi => "gpt-4o-mini",
            Self::Anthropic => "claude-3-5-haiku-latest",
        }
    }
}

/// One completion request, already resolved from a node's ports + params.
#[derive(Debug, Clone)]
pub struct AiRequest {
    pub provider: AiProviderKind,
    pub model: String,
    pub base_url: Option<String>,
    pub prompt: String,
    pub system: Option<String>,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u64>,
}

#[derive(Debug, Error)]
pub enum AiError {
    #[error("no API key configured for provider '{0}': add one in the toolbar's AI Keys dialog")]
    MissingKey(String),
    #[error("AI provider '{provider}' failed: {message}")]
    Provider { provider: String, message: String },
    #[error("secret store error: {0}")]
    Secrets(String),
}

/// The seam between the executor and whatever answers completions.
///
/// Implementations must be `Send + Sync` because the runner holds the
/// gateway in an `Arc` across await points.
pub trait AiGateway: Send + Sync {
    fn complete<'a>(&'a self, req: &'a AiRequest) -> BoxFuture<'a, Result<String, AiError>>;
}

/// Per-node context handed to an async executor. Owns its strings (and
/// shares the gateway) so the boxed future has no borrow of the runner's
/// per-iteration locals — the runner clones the inputs and drops them
/// before awaiting.
#[derive(Clone)]
pub struct AiEnv {
    pub node_id: String,
    pub gateway: Arc<dyn AiGateway>,
}

/// Gateway used when a caller runs a graph without supplying one
/// (`run_graph`). Every AI node errors loudly instead of silently
/// pretending to work, which is what tests and hypothetical non-Tauri
/// callers want.
#[allow(dead_code)] // reached through `run_graph`, which only tests call
pub struct UnconfiguredGateway;

impl AiGateway for UnconfiguredGateway {
    fn complete<'a>(&'a self, req: &'a AiRequest) -> BoxFuture<'a, Result<String, AiError>> {
        let provider = req.provider.as_str().to_string();
        Box::pin(async move { Err(AiError::MissingKey(provider)) })
    }
}

/// Production gateway: reads the API key from the on-disk [`SecretStore`]
/// and delegates the HTTP call to [`providers::complete`].
pub struct RigGateway {
    secrets: SecretStore,
}

impl RigGateway {
    pub fn new(secrets: SecretStore) -> Self {
        Self { secrets }
    }
}

impl AiGateway for RigGateway {
    fn complete<'a>(&'a self, req: &'a AiRequest) -> BoxFuture<'a, Result<String, AiError>> {
        Box::pin(async move {
            let provider = req.provider.as_str().to_string();
            let key = self
                .secrets
                .get(&provider)
                .map_err(AiError::Secrets)?
                .ok_or(AiError::MissingKey(provider.clone()))?;
            providers::complete(&key, req).await
        })
    }
}
