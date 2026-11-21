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
use serde_json::{json, Value};

use super::{
    AiError, AiProviderKind, AiRequest, ChatMessage, ChatTurn, ToolCall, ToolSpec,
};

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

/// Run one **vision** completion: prompt plus base64 images.
///
/// Kept separate from [`complete`] because rig's text completion has no
/// image support, and from [`complete_turn`] because a vision node does not
/// run a tool loop. Both wires get the same retry policy as text.
pub async fn complete_vision(api_key: &str, req: &AiRequest) -> Result<String, AiError> {
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        let outcome = match req.provider {
            AiProviderKind::OpenAi => openai_vision(api_key, req).await,
            AiProviderKind::Anthropic => anthropic_vision(api_key, req).await,
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
                let delay = 400u64 * 2u64.pow(attempt - 1);
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
        }
    }
}

/// Sniff the image MIME type from the leading base64 characters. Our
/// `binary` convention carries raw file bytes in base64, so the file magic
/// is preserved; an unknown payload is declared `image/png`, which is what
/// `image.format` emits by default.
fn image_media_type(data: &str) -> &'static str {
    let head: String = data.chars().take(16).collect();
    if head.starts_with("iVBORw0KGgo") {
        "image/png"
    } else if head.starts_with("/9j/") {
        "image/jpeg"
    } else if head.starts_with("R0lGOD") {
        "image/gif"
    } else if head.starts_with("UklGR") {
        "image/webp"
    } else {
        "image/png"
    }
}

fn openai_vision_payload(req: &AiRequest) -> Value {
    let mut content = vec![json!({ "type": "text", "text": &req.prompt })];
    for image in &req.images {
        content.push(json!({
            "type": "image_url",
            "image_url": {
                "url": format!("data:{};base64,{image}", image_media_type(image))
            }
        }));
    }

    let mut messages: Vec<Value> = Vec::new();
    if let Some(system) = req
        .system
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        messages.push(json!({ "role": "system", "content": system }));
    }
    messages.push(json!({ "role": "user", "content": content }));

    let mut payload = json!({ "model": &req.model, "messages": messages });
    if let Some(temperature) = req.temperature {
        payload["temperature"] = json!(temperature);
    }
    if let Some(max_tokens) = req.max_tokens {
        payload["max_tokens"] = json!(max_tokens);
    }
    payload
}

fn anthropic_vision_payload(req: &AiRequest) -> Value {
    let mut content: Vec<Value> = req
        .images
        .iter()
        .map(|image| {
            json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": image_media_type(image),
                    "data": image,
                }
            })
        })
        .collect();
    content.push(json!({ "type": "text", "text": &req.prompt }));

    let mut payload = json!({
        "model": &req.model,
        "max_tokens": req.max_tokens.unwrap_or(ANTHROPIC_DEFAULT_MAX_TOKENS),
        "messages": [{ "role": "user", "content": content }],
    });
    if let Some(system) = req
        .system
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        payload["system"] = json!(system);
    }
    if let Some(temperature) = req.temperature {
        payload["temperature"] = json!(temperature);
    }
    payload
}

async fn openai_vision(api_key: &str, req: &AiRequest) -> Result<String, String> {
    let base = req.base_url.as_deref().unwrap_or("https://api.openai.com/v1");
    let body = post_json(
        &openai_endpoint(base),
        vec![("Authorization", format!("Bearer {api_key}"))],
        &openai_vision_payload(req),
    )
    .await?;
    let text = body["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .to_string();
    if text.trim().is_empty() {
        return Err("provider returned no text content".to_string());
    }
    Ok(text)
}

async fn anthropic_vision(api_key: &str, req: &AiRequest) -> Result<String, String> {
    let base = req
        .base_url
        .as_deref()
        .unwrap_or("https://api.anthropic.com");
    let body = post_json(
        &anthropic_endpoint(base),
        vec![
            ("x-api-key", api_key.to_string()),
            ("anthropic-version", "2023-06-01".to_string()),
        ],
        &anthropic_vision_payload(req),
    )
    .await?;

    let blocks = body["content"]
        .as_array()
        .ok_or_else(|| "provider returned no content".to_string())?;
    let mut text = String::new();
    for block in blocks {
        if block["type"].as_str() == Some("text") {
            text.push_str(block["text"].as_str().unwrap_or(""));
        }
    }
    if text.trim().is_empty() {
        return Err("provider returned no text content".to_string());
    }
    Ok(text)
}

/// Shared POST helper for the raw-JSON paths. Retries are handled by the
/// callers, so this only reports transport/HTTP/JSON failures.
async fn post_json(
    endpoint: &str,
    headers: Vec<(&str, String)>,
    payload: &Value,
) -> Result<Value, String> {
    let mut request = reqwest::Client::new()
        .post(endpoint)
        .header("content-type", "application/json")
        .timeout(Duration::from_secs(120));
    for (name, value) in headers {
        request = request.header(name, value);
    }
    let response = request
        .json(payload)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let text = response.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!(
            "HTTP {}: {}",
            status.as_u16(),
            truncated(&text, 500)
        ));
    }
    serde_json::from_str(&text).map_err(|e| format!("invalid provider JSON: {e}"))
}

/// Run one **tool-aware** turn against `req`'s provider.
///
/// Text completion goes through rig; tool calling does not, because the
/// loop has to see `tool_calls` and to send the assistant/tool messages
/// back verbatim on the next turn. Both OpenAI-compatible and Anthropic
/// wires therefore use raw JSON here — the payloads below are the stable
/// public shape of each API, not a rig implementation detail.
///
/// Retries mirror [`complete`]: transient errors are attempted again.
pub async fn complete_turn(
    api_key: &str,
    req: &AiRequest,
    messages: &[ChatMessage],
    tools: &[ToolSpec],
) -> Result<ChatTurn, AiError> {
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        let outcome = match req.provider {
            AiProviderKind::OpenAi => openai_turn(api_key, req, messages, tools).await,
            AiProviderKind::Anthropic => anthropic_turn(api_key, req, messages, tools).await,
        };
        match outcome {
            Ok(turn) => return Ok(turn),
            Err(message) => {
                if attempt >= MAX_ATTEMPTS || !is_retryable(&message) {
                    return Err(AiError::Provider {
                        provider: req.provider.as_str().to_string(),
                        message,
                    });
                }
                let delay = 400u64 * 2u64.pow(attempt - 1);
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
        }
    }
}

fn openai_endpoint(base: &str) -> String {
    let base = base.trim().trim_end_matches('/');
    if base.ends_with("/chat/completions") {
        base.to_string()
    } else {
        format!("{base}/chat/completions")
    }
}

fn anthropic_endpoint(base: &str) -> String {
    let base = base.trim().trim_end_matches('/');
    if base.ends_with("/messages") {
        base.to_string()
    } else {
        format!("{base}/v1/messages")
    }
}

/// Tool arguments travel as a JSON *string* in the OpenAI wire.
fn arguments_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn openai_messages(messages: &[ChatMessage]) -> Vec<Value> {
    messages
        .iter()
        .map(|m| match m {
            ChatMessage::User(text) => json!({ "role": "user", "content": text }),
            ChatMessage::Assistant { text, tool_calls } => {
                if tool_calls.is_empty() {
                    return json!({ "role": "assistant", "content": text });
                }
                let calls: Vec<Value> = tool_calls
                    .iter()
                    .map(|c| {
                        json!({
                            "id": c.id,
                            "type": "function",
                            "function": {
                                "name": c.name,
                                "arguments": arguments_text(&c.arguments),
                            }
                        })
                    })
                    .collect();
                json!({
                    "role": "assistant",
                    "content": if text.trim().is_empty() { Value::Null } else { json!(text) },
                    "tool_calls": calls,
                })
            }
            ChatMessage::Tool {
                call_id, content, ..
            } => json!({ "role": "tool", "tool_call_id": call_id, "content": content }),
        })
        .collect()
}

/// Anthropic wants content blocks and alternating roles, so consecutive
/// same-role messages (tool results after a user turn, for instance) are
/// merged into one message.
fn anthropic_messages(messages: &[ChatMessage]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for message in messages {
        let (role, blocks) = match message {
            ChatMessage::User(text) => {
                ("user", vec![json!({ "type": "text", "text": text })])
            }
            ChatMessage::Assistant { text, tool_calls } => {
                let mut blocks: Vec<Value> = Vec::new();
                if !text.trim().is_empty() {
                    blocks.push(json!({ "type": "text", "text": text }));
                }
                for call in tool_calls {
                    blocks.push(json!({
                        "type": "tool_use",
                        "id": call.id,
                        "name": call.name,
                        "input": if call.arguments.is_null() { json!({}) } else { call.arguments.clone() },
                    }));
                }
                if blocks.is_empty() {
                    blocks.push(json!({ "type": "text", "text": "" }));
                }
                ("assistant", blocks)
            }
            ChatMessage::Tool {
                call_id, content, ..
            } => (
                "user",
                vec![json!({
                    "type": "tool_result",
                    "tool_use_id": call_id,
                    "content": content,
                })],
            ),
        };

        if let Some(last) = out.last_mut() {
            if last["role"].as_str() == Some(role) {
                if let Some(existing) = last["content"].as_array_mut() {
                    existing.extend(blocks);
                    continue;
                }
            }
        }
        out.push(json!({ "role": role, "content": blocks }));
    }
    out
}

fn truncated(text: &str, max: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max {
        return trimmed.to_string();
    }
    let head: String = trimmed.chars().take(max).collect();
    format!("{head}…")
}

async fn openai_turn(
    api_key: &str,
    req: &AiRequest,
    messages: &[ChatMessage],
    tools: &[ToolSpec],
) -> Result<ChatTurn, String> {
    let base = req.base_url.as_deref().unwrap_or("https://api.openai.com/v1");
    let endpoint = openai_endpoint(base);

    let mut payload = json!({
        "model": req.model,
        "messages": openai_messages(messages),
    });
    if !tools.is_empty() {
        payload["tools"] = Value::Array(
            tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters,
                        }
                    })
                })
                .collect(),
        );
    }
    if let Some(temperature) = req.temperature {
        payload["temperature"] = json!(temperature);
    }
    if let Some(max_tokens) = req.max_tokens {
        payload["max_tokens"] = json!(max_tokens);
    }
    if let Some(system) = req
        .system
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        // Chat Completions takes the system prompt as a message, unlike
        // rig's preamble helper which builds it for us.
        if let Some(list) = payload["messages"].as_array_mut() {
            list.insert(0, json!({ "role": "system", "content": system }));
        }
    }

    let response = reqwest::Client::new()
        .post(&endpoint)
        .bearer_auth(api_key)
        .header("content-type", "application/json")
        .timeout(Duration::from_secs(120))
        .json(&payload)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let text = response.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!(
            "HTTP {}: {}",
            status.as_u16(),
            truncated(&text, 500)
        ));
    }
    let body: Value = serde_json::from_str(&text)
        .map_err(|e| format!("invalid provider JSON: {e}"))?;
    let message = &body["choices"][0]["message"];
    let answer = message["content"].as_str().unwrap_or("").to_string();

    let mut tool_calls = Vec::new();
    if let Some(calls) = message["tool_calls"].as_array() {
        for call in calls {
            let name = call["function"]["name"].as_str().unwrap_or("").to_string();
            if name.is_empty() {
                continue;
            }
            let raw = &call["function"]["arguments"];
            let arguments = match raw {
                Value::String(s) => serde_json::from_str::<Value>(s)
                    .unwrap_or_else(|_| Value::String(s.clone())),
                Value::Null => Value::Null,
                other => other.clone(),
            };
            tool_calls.push(ToolCall {
                id: call["id"].as_str().unwrap_or("").to_string(),
                name,
                arguments,
            });
        }
    }
    if answer.trim().is_empty() && tool_calls.is_empty() {
        return Err("provider returned no content".to_string());
    }
    Ok(ChatTurn {
        text: answer,
        tool_calls,
    })
}

async fn anthropic_turn(
    api_key: &str,
    req: &AiRequest,
    messages: &[ChatMessage],
    tools: &[ToolSpec],
) -> Result<ChatTurn, String> {
    let base = req
        .base_url
        .as_deref()
        .unwrap_or("https://api.anthropic.com");
    let endpoint = anthropic_endpoint(base);

    let mut payload = json!({
        "model": req.model,
        "max_tokens": req.max_tokens.unwrap_or(ANTHROPIC_DEFAULT_MAX_TOKENS),
        "messages": anthropic_messages(messages),
    });
    if !tools.is_empty() {
        payload["tools"] = Value::Array(
            tools
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.parameters,
                    })
                })
                .collect(),
        );
    }
    if let Some(temperature) = req.temperature {
        payload["temperature"] = json!(temperature);
    }
    if let Some(system) = req
        .system
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        payload["system"] = json!(system);
    }

    let response = reqwest::Client::new()
        .post(&endpoint)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .timeout(Duration::from_secs(120))
        .json(&payload)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let text = response.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!(
            "HTTP {}: {}",
            status.as_u16(),
            truncated(&text, 500)
        ));
    }
    let body: Value = serde_json::from_str(&text)
        .map_err(|e| format!("invalid provider JSON: {e}"))?;
    let blocks = body["content"]
        .as_array()
        .ok_or_else(|| "provider returned no content".to_string())?;

    let mut answer = String::new();
    let mut tool_calls = Vec::new();
    for block in blocks {
        match block["type"].as_str() {
            Some("text") => answer.push_str(block["text"].as_str().unwrap_or("")),
            Some("tool_use") => {
                let name = block["name"].as_str().unwrap_or("").to_string();
                if name.is_empty() {
                    continue;
                }
                tool_calls.push(ToolCall {
                    id: block["id"].as_str().unwrap_or("").to_string(),
                    name,
                    arguments: block["input"].clone(),
                });
            }
            _ => {}
        }
    }
    if answer.trim().is_empty() && tool_calls.is_empty() {
        return Err("provider returned no content".to_string());
    }
    Ok(ChatTurn {
        text: answer,
        tool_calls,
    })
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
            provider_id: "anthropic".into(),
            provider: AiProviderKind::Anthropic,
            model: "claude-3-5-haiku-latest".into(),
            base_url: None,
            prompt: "hi".into(),
            system: None,
            temperature: None,
            max_tokens: None,
            images: Vec::new(),
        };
        let built = build_request(&req, &req.prompt);
        assert_eq!(built.max_tokens, Some(ANTHROPIC_DEFAULT_MAX_TOKENS));
    }

    #[test]
    fn endpoints_accept_a_base_url_or_a_full_endpoint() {
        assert_eq!(
            openai_endpoint("http://127.0.0.1:11434/v1/"),
            "http://127.0.0.1:11434/v1/chat/completions"
        );
        assert_eq!(
            openai_endpoint("https://host/v1/chat/completions"),
            "https://host/v1/chat/completions"
        );
        assert_eq!(
            anthropic_endpoint("https://api.anthropic.com"),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn media_types_are_sniffed_from_the_magic_bytes() {
        assert_eq!(image_media_type("iVBORw0KGgoAAAA"), "image/png");
        assert_eq!(image_media_type("/9j/4AAQSkZJRg"), "image/jpeg");
        assert_eq!(image_media_type("R0lGODlhAQAB"), "image/gif");
        assert_eq!(image_media_type("UklGRiQAAABXRUJQ"), "image/webp");
        // Unknown payloads fall back to the encoder's default.
        assert_eq!(image_media_type("AAAA"), "image/png");
    }

    fn vision_request() -> AiRequest {
        AiRequest {
            provider_id: "openai".into(),
            provider: AiProviderKind::OpenAi,
            model: "gpt-4o".into(),
            base_url: None,
            prompt: "what is this?".into(),
            system: Some("Be brief.".into()),
            temperature: Some(0.1),
            max_tokens: Some(64),
            images: vec!["iVBORw0KGgoAAAA".into(), "/9j/4AAQSkZJRg".into()],
        }
    }

    #[test]
    fn openai_vision_sends_data_urls_and_a_system_message() {
        let payload = openai_vision_payload(&vision_request());
        assert_eq!(payload["messages"][0]["role"], "system");
        let content = payload["messages"][1]["content"].as_array().unwrap();
        assert_eq!(content[0]["text"], "what is this?");
        assert_eq!(
            content[1]["image_url"]["url"],
            "data:image/png;base64,iVBORw0KGgoAAAA"
        );
        assert_eq!(
            content[2]["image_url"]["url"],
            "data:image/jpeg;base64,/9j/4AAQSkZJRg"
        );
    }

    #[test]
    fn anthropic_vision_uses_base64_blocks() {
        let payload = anthropic_vision_payload(&vision_request());
        assert_eq!(payload["system"], "Be brief.");
        let content = payload["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "image");
        assert_eq!(content[0]["source"]["media_type"], "image/png");
        assert_eq!(content[2]["type"], "text");
        assert_eq!(content[2]["text"], "what is this?");
    }

    #[test]
    fn openai_messages_round_trip_tool_calls() {
        let messages = vec![
            ChatMessage::User("hi".into()),
            ChatMessage::Assistant {
                text: String::new(),
                tool_calls: vec![ToolCall {
                    id: "call_1".into(),
                    name: "lookup".into(),
                    arguments: json!({"id": 7}),
                }],
            },
            ChatMessage::Tool {
                call_id: "call_1".into(),
                name: "lookup".into(),
                content: "{\"ok\":true}".into(),
            },
        ];
        let wire = openai_messages(&messages);
        assert_eq!(wire[1]["tool_calls"][0]["function"]["name"], "lookup");
        // OpenAI wants arguments as a JSON *string*.
        assert_eq!(
            wire[1]["tool_calls"][0]["function"]["arguments"],
            "{\"id\":7}"
        );
        assert_eq!(wire[2]["role"], "tool");
        assert_eq!(wire[2]["tool_call_id"], "call_1");
    }

    #[test]
    fn anthropic_merges_consecutive_tool_results_into_one_user_turn() {
        let messages = vec![
            ChatMessage::User("hi".into()),
            ChatMessage::Assistant {
                text: String::new(),
                tool_calls: vec![
                    ToolCall { id: "a".into(), name: "x".into(), arguments: json!({}) },
                    ToolCall { id: "b".into(), name: "y".into(), arguments: json!({}) },
                ],
            },
            ChatMessage::Tool {
                call_id: "a".into(),
                name: "x".into(),
                content: "1".into(),
            },
            ChatMessage::Tool {
                call_id: "b".into(),
                name: "y".into(),
                content: "2".into(),
            },
        ];
        let wire = anthropic_messages(&messages);
        assert_eq!(wire.len(), 3);
        assert_eq!(wire[2]["role"], "user");
        assert_eq!(wire[2]["content"].as_array().unwrap().len(), 2);
        assert_eq!(wire[2]["content"][0]["type"], "tool_result");
    }

    #[test]
    fn system_prompt_lands_in_the_preamble() {
        let req = AiRequest {
            provider_id: "openai".into(),
            provider: AiProviderKind::OpenAi,
            model: "gpt-4o-mini".into(),
            base_url: None,
            prompt: "hi".into(),
            system: Some("Be terse.".into()),
            temperature: Some(0.1),
            max_tokens: Some(64),
            images: Vec::new(),
        };
        let built = build_request(&req, &req.prompt);
        assert_eq!(built.system_instructions(), Some("Be terse."));
        assert_eq!(built.temperature, Some(0.1));
        assert_eq!(built.max_tokens, Some(64));
    }
}
