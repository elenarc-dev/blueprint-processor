//! `ai.completion` — sends a prompt to an external model and emits the
//! reply as a string.
//!
//! Shape mirroring: a scalar prompt produces a scalar answer; an array of
//! prompts produces a parallel array of answers (**one call per element**),
//! up to `params.concurrency` in flight. That matches the
//! `string.replace` / `file.read` convention already used by the catalog,
//! so an `ai.completion` node can sit in the middle of a dataset pipeline
//! without a wrapper node.
//!
//! This is the only node whose executor is async. It is registered in
//! `setup_async_registry()` (not `setup_registry()`) and therefore takes
//! **owned** inputs: the runner clones them so the boxed future never
//! borrows the per-iteration locals.
//!
//! Credentials are never read from `params`. The gateway resolves the API
//! key from the app's secret store, keyed by provider.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use futures::stream::{self, StreamExt, TryStreamExt};
use serde_json::{Map, Value};

use crate::ai::{AiEnv, AiProviderKind, AiRequest};
use crate::executor::{single, value_to_string, ExecError, NodeOutputs};

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
    let provider_raw = params
        .get("provider")
        .and_then(Value::as_str)
        .unwrap_or("openai");
    let provider = AiProviderKind::parse(provider_raw).ok_or_else(|| ExecError::Node {
        node: env.node_id.clone(),
        message: format!(
            "unknown AI provider '{provider_raw}' (expected one of: {})",
            AiProviderKind::ALL.join(", ")
        ),
    })?;

    let model = params
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| provider.default_model().to_string());

    let base_url = params
        .get("base_url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let system = params
        .get("system")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let temperature = params.get("temperature").and_then(Value::as_f64);
    let max_tokens = params.get("max_tokens").and_then(Value::as_u64);
    let concurrency = params
        .get("concurrency")
        .and_then(Value::as_u64)
        .unwrap_or(4)
        .clamp(1, 16) as usize;

    // The runner already resolves the unwired-port fallback to
    // `params["prompt"]`, so one lookup covers both "wired" and "typed in
    // the node body".
    let prompt_value = inputs.get("prompt").cloned().unwrap_or(Value::Null);
    let (mirror_shape, prompts): (bool, Vec<String>) = match &prompt_value {
        Value::String(s) if !s.trim().is_empty() => (false, vec![s.clone()]),
        Value::Array(items) if !items.is_empty() => {
            (true, items.iter().map(value_to_string).collect())
        }
        _ => {
            return Err(ExecError::Node {
                node: env.node_id.clone(),
                message:
                    "prompt is required: wire the 'Prompt' port or set the 'Prompt (default)' param"
                        .into(),
            })
        }
    };

    let requests: Vec<AiRequest> = prompts
        .into_iter()
        .map(|prompt| AiRequest {
            provider,
            model: model.clone(),
            base_url: base_url.clone(),
            prompt,
            system: system.clone(),
            temperature,
            max_tokens,
        })
        .collect();

    // `buffered` (not `buffer_unordered`) keeps the output order aligned
    // with the input array while still allowing `concurrency` in flight.
    let answers: Vec<String> = stream::iter(requests.into_iter().map(|req| {
        let gateway = Arc::clone(&env.gateway);
        async move { gateway.complete(&req).await }
    }))
    .buffered(concurrency)
    .try_collect()
    .await
    .map_err(|e| ExecError::Node {
        node: env.node_id.clone(),
        message: e.to_string(),
    })?;

    if mirror_shape {
        Ok(single(
            "out",
            Value::Array(answers.into_iter().map(Value::String).collect()),
        ))
    } else {
        Ok(single(
            "out",
            Value::String(answers.into_iter().next().unwrap_or_default()),
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use serde_json::{json, Map, Value};

    use super::exec;
    use crate::ai::mock::MockGateway;
    use crate::ai::AiEnv;

    fn upper(prompt: &str) -> String {
        prompt.to_uppercase()
    }

    fn params(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    fn env(gateway: Arc<MockGateway>) -> AiEnv {
        AiEnv {
            node_id: "ai1".into(),
            gateway,
        }
    }

    fn call(
        inputs: Map<String, Value>,
        params: Map<String, Value>,
        env: &AiEnv,
    ) -> Result<HashMap<String, Value>, crate::executor::ExecError> {
        tauri::async_runtime::block_on(exec(&inputs, &params, env))
    }

    #[test]
    fn scalar_prompt_produces_scalar_out() {
        let gateway = Arc::new(MockGateway::new(upper));
        let env = env(gateway.clone());
        let mut inputs = Map::new();
        inputs.insert("prompt".into(), json!("hello"));

        let outs = call(inputs, params(json!({ "model": "m" })), &env).unwrap();
        assert_eq!(outs.get("out").unwrap().as_str(), Some("HELLO"));
        assert_eq!(gateway.calls().len(), 1);
    }

    #[test]
    fn array_prompt_mirrors_shape_and_keeps_order() {
        let gateway = Arc::new(MockGateway::new(upper));
        let env = env(gateway.clone());
        let mut inputs = Map::new();
        inputs.insert("prompt".into(), json!(["a", "b", "c"]));

        let outs = call(inputs, params(json!({ "concurrency": 3 })), &env).unwrap();
        let arr = outs.get("out").unwrap().as_array().unwrap();
        assert_eq!(
            arr.iter().map(|v| v.as_str().unwrap()).collect::<Vec<_>>(),
            vec!["A", "B", "C"]
        );
        assert_eq!(gateway.calls().len(), 3);
    }

    #[test]
    fn unwired_prompt_falls_back_to_params() {
        let gateway = Arc::new(MockGateway::new(upper));
        let env = env(gateway.clone());
        // The runner resolves the unwired port to `params.prompt`; this
        // test pins that the executor reads exactly that.
        let mut inputs = Map::new();
        inputs.insert("prompt".into(), json!("typed in body"));

        let outs = call(inputs, params(json!({})), &env).unwrap();
        assert_eq!(outs.get("out").unwrap().as_str(), Some("TYPED IN BODY"));
    }

    #[test]
    fn missing_prompt_errors() {
        let gateway = Arc::new(MockGateway::new(upper));
        let env = env(gateway);
        let mut inputs = Map::new();
        inputs.insert("prompt".into(), Value::Null);

        let err = call(inputs, params(json!({})), &env).unwrap_err();
        assert!(err.to_string().contains("prompt is required"));
    }

    #[test]
    fn unknown_provider_errors_before_calling_out() {
        let gateway = Arc::new(MockGateway::new(upper));
        let env = env(gateway.clone());
        let mut inputs = Map::new();
        inputs.insert("prompt".into(), json!("hi"));

        let err = call(inputs, params(json!({ "provider": "nope" })), &env).unwrap_err();
        assert!(err.to_string().contains("unknown AI provider"));
        assert!(gateway.calls().is_empty());
    }
}
