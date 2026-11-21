//! `http.request` — call an HTTP endpoint and surface the response.
//!
//! This is the only node besides `ai.*` that awaits I/O, so it lives in the
//! async registry. It is also the transport used by app-level AI tools
//! (see [`perform`]).
//!
//! Outputs:
//!   * `status` — HTTP status code (a 4xx/5xx is data, not a node failure).
//!   * `body`   — response body as text.
//!   * `json`   — the body parsed as JSON, or `null` when it is not JSON.
//!
//! The request body is either a JSON string (sent verbatim, so the user can
//! send a pre-serialized payload) or any other JSON value (serialized as
//! JSON with `content-type: application/json`).
//!
//! Connection-level failures (bad URL, DNS, TLS, timeout) do fail the node:
//! there is no status code to surface.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::ai::AiEnv;
use crate::executor::{ExecError, NodeOutputs};

/// What one HTTP round-trip produced.
#[derive(Debug)]
pub struct HttpOutcome {
    pub status: u16,
    pub text: String,
    pub json: Value,
}

/// Parse a `Name: Value` per-line header block.
///
/// Blank lines are ignored. A line without a colon is an error mentioning
/// the line number so a typo is easy to locate.
pub fn parse_headers(raw: &str) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for (i, line) in raw.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(format!(
                "header line {} is not 'Name: Value': {line:?}",
                i + 1
            ));
        };
        let name = name.trim();
        if name.is_empty() {
            return Err(format!("header line {} has an empty name", i + 1));
        }
        out.push((name.to_string(), value.trim().to_string()));
    }
    Ok(out)
}

/// Perform one request. Shared by the `http.request` node and by the
/// `ai.completion` node when it executes an app-level tool.
pub async fn perform(
    method: &str,
    url: &str,
    headers: &str,
    body: Option<&Value>,
    timeout_ms: u64,
) -> Result<HttpOutcome, String> {
    let method_raw = method.trim().to_ascii_uppercase();
    let method = reqwest::Method::from_bytes(method_raw.as_bytes())
        .map_err(|_| format!("invalid HTTP method '{method_raw}'"))?;
    let url = url.trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("url must start with http:// or https://".into());
    }

    let pairs = parse_headers(headers)?;
    let timeout = Duration::from_millis(timeout_ms.clamp(100, 600_000));
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| format!("build HTTP client: {e}"))?;

    let mut request = client.request(method, url);
    for (name, value) in pairs {
        request = request.header(name, value);
    }
    if let Some(body) = body {
        if !body.is_null() {
            request = match body {
                Value::String(text) => request.body(text.clone()),
                other => request.json(other),
            };
        }
    }

    let response = request
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    let status = response.status().as_u16();
    let text = response
        .text()
        .await
        .map_err(|e| format!("read response body: {e}"))?;
    let json = serde_json::from_str(&text).unwrap_or(Value::Null);
    Ok(HttpOutcome { status, text, json })
}

pub fn exec<'a>(
    inputs: &'a Map<String, Value>,
    params: &'a Map<String, Value>,
    env: &'a AiEnv,
) -> Pin<Box<dyn Future<Output = Result<NodeOutputs, ExecError>> + Send + 'a>> {
    Box::pin(run(inputs, params, env))
}

async fn run(
    inputs: &Map<String, Value>,
    params: &Map<String, Value>,
    env: &AiEnv,
) -> Result<NodeOutputs, ExecError> {
    let method = params
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("GET");
    let headers = params
        .get("headers")
        .and_then(Value::as_str)
        .unwrap_or("");
    let timeout_ms = params
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .unwrap_or(30_000);

    // The runner already substitutes `params.url` for an unwired port.
    let url = match inputs.get("url") {
        Some(Value::String(s)) => s.trim().to_string(),
        _ => String::new(),
    };
    if url.is_empty() {
        return Err(ExecError::Node {
            node: env.node_id.clone(),
            message: "url is required: wire the 'URL' port or type it in the node".into(),
        });
    }

    let body = inputs.get("body").cloned();
    let outcome = perform(method, &url, headers, body.as_ref(), timeout_ms)
        .await
        .map_err(|message| ExecError::Node {
            node: env.node_id.clone(),
            message,
        })?;

    let mut outs = NodeOutputs::new();
    outs.insert("then_out".into(), Value::Null);
    outs.insert("status".into(), json!(outcome.status));
    outs.insert("body".into(), Value::String(outcome.text));
    outs.insert("json".into(), outcome.json);
    Ok(outs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_parse_blank_lines_and_whitespace() {
        let pairs = parse_headers(" Accept : application/json \n\nX-Test: 1 ").unwrap();
        assert_eq!(
            pairs,
            vec![
                ("Accept".to_string(), "application/json".to_string()),
                ("X-Test".to_string(), "1".to_string()),
            ]
        );
    }

    #[test]
    fn malformed_header_names_the_line() {
        let err = parse_headers("Accept: x\nnot a header").unwrap_err();
        assert!(err.contains("line 2"), "{err}");
    }

    #[test]
    fn empty_url_fails_without_touching_the_network() {
        let env = AiEnv {
            node_id: "http1".into(),
            gateway: std::sync::Arc::new(crate::ai::UnconfiguredGateway),
            depth: 0,
        };
        let inputs = Map::new();
        let mut params = Map::new();
        params.insert("url".into(), json!(""));
        let err = tauri::async_runtime::block_on(exec(&inputs, &params, &env)).unwrap_err();
        assert!(err.to_string().contains("url is required"), "{err}");
    }

    #[test]
    fn non_http_schemes_are_rejected() {
        let err = tauri::async_runtime::block_on(perform("GET", "ftp://x", "", None, 1000))
            .unwrap_err();
        assert!(err.contains("http://"), "{err}");
    }
}
