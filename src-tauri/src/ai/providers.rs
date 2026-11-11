//! Provider translation layer.
//!
//! **The only module that depends on `rig-core`.** Everything else in the
//! backend talks to [`crate::ai::AiGateway`], so a breaking change in Rig
//! (it is pre-1.0 and has rewritten its completion API more than once)
//! stays inside this file.
//!
//! Retries: a single completion has no retry logic in Rig that we can
//! rely on across providers, and a long blueprint run should not die
//! because one request hit a transient 429/5xx. We retry up to
//! [`MAX_ATTEMPTS`] with a small fixed backoff (400 ms, 800 ms) and only
//! on errors whose message looks retryable. Auth/validation errors fail
//! immediately — retrying a bad key just wastes time.

use std::time::Duration;

use rig_core::completion::message::AssistantContent;
use rig_core::completion::CompletionRequest;
use rig_core::providers::{anthropic, openai};

use super::{AiError, AiProviderKind, AiRequest};

/// Total attempts per completion, including the first one.
const MAX_ATTEMPTS: u32 = 3;

/// Anthropic's Messages API requires an output-token cap. When the node
/// leaves `max_tokens` empty we send this instead of letting the encoder
/// reject the request.
const ANTHROPIC_DEFAULT_MAX_TOKENS: u64 = 1024;

/// Run one completion for `req`, retrying transient failures.
pub async fn complete(api_key: &str, req: &AiRequest) -> Result<String, AiError> {
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        let outcome = match req.provider {
            AiProviderKind::OpenAi => openai_completion(api_key, req).await,
            AiProviderKind::Anthropic => anthropic_completion(api_key, req).await,
        };
        match outcome {
            Ok(text) => return Ok(text),
            Err(message) => {
                if attempt >= MAX_ATTEMPTS || !is_retryable(&message) {
                    return Err(AiError::Provider {
                        provider: req.provider.as_str().to_string(),
                        message,
                    });
                }
                // 400 ms then 800 ms. No jitter: a blueprint run stays
                // close to deterministic, which matters for tests and for
                // users diffing two runs.
                let delay = 400u64 * 2u64.pow(attempt - 1);
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
        }
    }
}

/// Heuristic on the provider error text. Rig normalises provider errors
/// into typed variants but the wording/payload differs per wire; matching
/// on the rendered message is the version-proof option and errs on the
/// side of not retrying what we don't recognise.
fn is_retryable(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    [
        "429",
        "500",
        "502",
        "503",
        "504",
        "rate limit",
        "too many requests",
        "overloaded",
        "timed out",
        "timeout",
        "connection reset",
        "temporarily unavailable",
    ]
    .iter()
    .any(|needle| m.contains(needle))
}

fn build_request(req: &AiRequest, prompt: &str) -> CompletionRequest {
    let mut request = CompletionRequest::new(prompt.to_string());
    if let Some(system) = req
        .system
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        request = request.preamble(system.to_string());
    }
    if let Some(temperature) = req.temperature {
        request = request.temperature(temperature);
    }
    let max_tokens = match (req.provider, req.max_tokens) {
        (AiProviderKind::Anthropic, None) => Some(ANTHROPIC_DEFAULT_MAX_TOKENS),
        (_, max) => max,
    };
    if let Some(max_tokens) = max_tokens {
        request = request.max_tokens(max_tokens);
    }
    request
}

async fn openai_completion(api_key: &str, req: &AiRequest) -> Result<String, String> {
    let mut config = openai::OpenAIConfig::new(api_key)
        // Chat Completions (not the Responses endpoint) is what every
        // OpenAI-compatible gateway implements, so `base_url` stays
        // usable for Ollama / LM Studio / Groq / OpenRouter.
        .with_route(openai::Route::Chat);
    if let Some(base_url) = req.base_url.as_deref() {
        config = config.with_base_url(base_url);
    }
    let model = config.client().completion(req.model.clone());
    let response = model
        .call(build_request(req, &req.prompt))
        .await
        .map_err(|e| e.to_string())?;
    collect_text(response.choice.iter())
}

async fn anthropic_completion(api_key: &str, req: &AiRequest) -> Result<String, String> {
    let mut config = anthropic::AnthropicConfig::new(api_key);
    if let Some(base_url) = req.base_url.as_deref() {
        config = config.with_base_url(base_url);
    }
    let model = config.client().completion(req.model.clone());
    let response = model
        .call(build_request(req, &req.prompt))
        .await
        .map_err(|e| e.to_string())?;
    collect_text(response.choice.iter())
}

/// Concatenate the text blocks of a reply. Reasoning / tool-call blocks
/// are ignored: an `ai.completion` node is a text transformer, not an
/// agent, and silently stringifying a tool call would leak JSON into the
/// data flow.
fn collect_text<'a>(
    blocks: impl Iterator<Item = &'a AssistantContent>,
) -> Result<String, String> {
    let mut out = String::new();
    for block in blocks {
        if let AssistantContent::Text(text) = block {
            out.push_str(&text.text);
        }
    }
    if out.trim().is_empty() {
        return Err("provider returned no text content".to_string());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryable_errors_are_recognised() {
        assert!(is_retryable("HTTP 429 Too Many Requests"));
        assert!(is_retryable("rate limit exceeded"));
        assert!(is_retryable("503 Service Unavailable"));
        assert!(!is_retryable("401 Unauthorized: invalid api key"));
        assert!(!is_retryable("unknown model 'nope'"));
    }

    #[test]
    fn anthropic_gets_a_default_max_tokens() {
        let req = AiRequest {
            provider: AiProviderKind::Anthropic,
            model: "claude-3-5-haiku-latest".into(),
            base_url: None,
            prompt: "hi".into(),
            system: None,
            temperature: None,
            max_tokens: None,
        };
        let built = build_request(&req, &req.prompt);
        assert_eq!(built.max_tokens, Some(ANTHROPIC_DEFAULT_MAX_TOKENS));
    }

    #[test]
    fn system_prompt_lands_in_the_preamble() {
        let req = AiRequest {
            provider: AiProviderKind::OpenAi,
            model: "gpt-4o-mini".into(),
            base_url: None,
            prompt: "hi".into(),
            system: Some("Be terse.".into()),
            temperature: Some(0.1),
            max_tokens: Some(64),
        };
        let built = build_request(&req, &req.prompt);
        assert_eq!(built.system_instructions(), Some("Be terse."));
        assert_eq!(built.temperature, Some(0.1));
        assert_eq!(built.max_tokens, Some(64));
    }
}
