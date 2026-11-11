//! `ai.completion` — sends a prompt (optionally combined with data) to an
//! external model and emits the reply as text.
//!
//! Two data inputs:
//!   * `prompt` — the instruction. A wired value wins; otherwise the
//!     `Prompt (default)` param is used (the runner already resolves an
//!     unwired port to `params[<port_id>]`).
//!   * `data` — optional payload from an upstream node.
//!
//! ## How prompt and data are combined
//!
//! * Prompt contains `{{data}}` → every occurrence is replaced with the
//!   serialized data value (objects and arrays become JSON text, so an
//!   `array.getField` → `array.create` pair can feed a structured row).
//! * Prompt has no placeholder → the data is appended after a blank line,
//!   which is the least surprising default for "here is the instruction,
//!   here is the material".
//! * No data connected → the prompt is sent unchanged. A prompt that
//!   still contains `{{data}}` is an **error**: sending a template with a
//!   hole in it produces a confidently wrong answer, which is worse than
//!   failing.
//!
//! ## Shape mirroring (broadcast rules)
//!
//! The same rules the rest of the catalog uses, applied to the pair
//! (prompt, data):
//!
//! | prompt     | data       | calls              |
//! |------------|------------|--------------------|
//! | scalar     | scalar     | 1                  |
//! | array(N)   | scalar     | N                  |
//! | scalar     | array(N)   | N                  |
//! | array(N)   | array(N)   | N (zipped)         |
//! | array(N)   | array(M≠N) | error              |
//!
//! The output is an array whenever either input was an array — a
//! 1-element array stays a 1-element array — and order is preserved
//! (`buffered`, not `buffer_unordered`).
//!
//! **An array data value means "one call per element".** To send a whole
//! array as a single JSON payload, route it through `json.stringify`
//! first; that turns it into a scalar string and keeps it as one value.
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

/// Placeholder for the `data` input inside the prompt text.
const DATA_PLACEHOLDER: &str = "{{data}}";

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
    // `data` has no param fallback on purpose: it is either wired to an
    // upstream node or absent, never typed by hand.
    let data_value = inputs.get("data").cloned().unwrap_or(Value::Null);

    let plan = pair_prompt_and_data(&prompt_value, &data_value, &env.node_id)?;

    let requests: Vec<AiRequest> = plan
        .items
        .iter()
        .map(|(prompt, data)| {
            Ok(AiRequest {
                provider,
                model: model.clone(),
                base_url: base_url.clone(),
                prompt: compose_prompt(prompt, data.as_ref(), &env.node_id)?,
                system: system.clone(),
                temperature,
                max_tokens,
            })
        })
        .collect::<Result<_, ExecError>>()?;

    // `buffered` (not `buffer_unordered`) keeps the output order aligned
    // with the input order while still allowing `concurrency` in flight.
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

    if plan.mirror {
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

/// One resolved `(prompt, data)` pair, plus whether the output must be an
/// array.
struct Plan {
    items: Vec<(String, Option<Value>)>,
    mirror: bool,
}

/// Resolve the cardinality of `(prompt, data)` into 1..N calls.
fn pair_prompt_and_data(
    prompt: &Value,
    data: &Value,
    node: &str,
) -> Result<Plan, ExecError> {
    let (prompt_mirror, prompts) = match prompt {
        Value::String(s) if !s.trim().is_empty() => (false, vec![s.clone()]),
        Value::Array(items) if !items.is_empty() => {
            (true, items.iter().map(value_to_string).collect())
        }
        _ => {
            return Err(ExecError::Node {
                node: node.to_string(),
                message:
                    "prompt is required: wire the 'Prompt' port or set the 'Prompt (default)' param"
                        .into(),
            })
        }
    };

    // `Null` / empty string / empty object-ish values count as "no data".
    let data_opt: Option<(bool, Vec<Value>)> = match data {
        Value::Null => None,
        Value::String(s) if s.is_empty() => None,
        Value::Array(items) => Some((true, items.clone())),
        other => Some((false, vec![other.clone()])),
    };

    let Some((data_mirror, datas)) = data_opt else {
        return Ok(Plan {
            mirror: prompt_mirror,
            items: prompts.into_iter().map(|p| (p, None)).collect(),
        });
    };

    let mirror = prompt_mirror || data_mirror;

    // An empty data array means "zero rows in", so zero calls out. The
    // shape still mirrors (empty array), which keeps a dataset pipeline
    // honest instead of silently falling back to one untyped call.
    if datas.is_empty() {
        return Ok(Plan {
            mirror: true,
            items: Vec::new(),
        });
    }

    if prompts.len() == 1 {
        let prompt = prompts[0].clone();
        return Ok(Plan {
            mirror,
            items: datas.into_iter().map(|d| (prompt.clone(), Some(d))).collect(),
        });
    }
    if datas.len() == 1 {
        let data = datas[0].clone();
        return Ok(Plan {
            mirror,
            items: prompts
                .into_iter()
                .map(|p| (p, Some(data.clone())))
                .collect(),
        });
    }
    if prompts.len() == datas.len() {
        return Ok(Plan {
            mirror,
            items: prompts
                .into_iter()
                .zip(datas)
                .map(|(p, d)| (p, Some(d)))
                .collect(),
        });
    }

    Err(ExecError::Node {
        node: node.to_string(),
        message: format!(
            "prompt array ({}) and data array ({}) must have the same length",
            prompts.len(),
            datas.len()
        ),
    })
}

/// Merge the prompt text with one data value.
fn compose_prompt(
    prompt: &str,
    data: Option<&Value>,
    node: &str,
) -> Result<String, ExecError> {
    let Some(value) = data else {
        if prompt.contains(DATA_PLACEHOLDER) {
            return Err(ExecError::Node {
                node: node.to_string(),
                message: format!(
                    "the prompt uses {DATA_PLACEHOLDER} but the 'Data' port is empty: connect a value or remove the placeholder"
                ),
            });
        }
        return Ok(prompt.to_string());
    };

    // `value_to_string` renders objects/arrays as JSON, which is what a
    // model should see for structured input.
    let text = value_to_string(value);

    if prompt.contains(DATA_PLACEHOLDER) {
        return Ok(prompt.replace(DATA_PLACEHOLDER, &text));
    }
    if prompt.trim().is_empty() {
        return Ok(text);
    }
    Ok(format!("{prompt}\n\n{text}"))
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

    fn echo(prompt: &str) -> String {
        prompt.to_string()
    }

    fn params(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    fn env_of(gateway: Arc<MockGateway>) -> AiEnv {
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

    fn inputs(prompt: Value, data: Value) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("prompt".into(), prompt);
        m.insert("data".into(), data);
        m
    }

    #[test]
    fn scalar_prompt_produces_scalar_out() {
        let gateway = Arc::new(MockGateway::new(upper));
        let env = env_of(gateway.clone());

        let outs = call(
            inputs(json!("hello"), Value::Null),
            params(json!({ "model": "m" })),
            &env,
        )
        .unwrap();
        assert_eq!(outs.get("out").unwrap().as_str(), Some("HELLO"));
        assert_eq!(gateway.calls().len(), 1);
    }

    #[test]
    fn array_prompt_mirrors_shape_and_keeps_order() {
        let gateway = Arc::new(MockGateway::new(upper));
        let env = env_of(gateway.clone());

        let outs = call(
            inputs(json!(["a", "b", "c"]), Value::Null),
            params(json!({ "concurrency": 3 })),
            &env,
        )
        .unwrap();
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
        let env = env_of(gateway.clone());
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
        let env = env_of(gateway);
        let mut inputs = Map::new();
        inputs.insert("prompt".into(), Value::Null);

        let err = call(inputs, params(json!({})), &env).unwrap_err();
        assert!(err.to_string().contains("prompt is required"));
    }

    #[test]
    fn unknown_provider_errors_before_calling_out() {
        let gateway = Arc::new(MockGateway::new(upper));
        let env = env_of(gateway.clone());
        let mut inputs = Map::new();
        inputs.insert("prompt".into(), json!("hi"));

        let err = call(inputs, params(json!({ "provider": "nope" })), &env).unwrap_err();
        assert!(err.to_string().contains("unknown AI provider"));
        assert!(gateway.calls().is_empty());
    }

    // ---------- data input ----------

    #[test]
    fn data_is_appended_after_a_blank_line() {
        let gateway = Arc::new(MockGateway::new(echo));
        let env = env_of(gateway.clone());

        let outs = call(
            inputs(json!("Summarize:"), json!("one, two")),
            params(json!({})),
            &env,
        )
        .unwrap();
        assert_eq!(
            outs.get("out").unwrap().as_str(),
            Some("Summarize:\n\none, two")
        );
        assert_eq!(gateway.calls().len(), 1);
    }

    #[test]
    fn placeholder_is_replaced_in_place() {
        let gateway = Arc::new(MockGateway::new(echo));
        let env = env_of(gateway.clone());

        let outs = call(
            inputs(
                json!("Translate to English: {{data}} (keep it short)"),
                json!("hola"),
            ),
            params(json!({})),
            &env,
        )
        .unwrap();
        assert_eq!(
            outs.get("out").unwrap().as_str(),
            Some("Translate to English: hola (keep it short)")
        );
    }

    #[test]
    fn every_placeholder_occurrence_is_replaced() {
        let gateway = Arc::new(MockGateway::new(echo));
        let env = env_of(gateway.clone());

        let outs = call(
            inputs(json!("{{data}} vs {{data}}"), json!("X")),
            params(json!({})),
            &env,
        )
        .unwrap();
        assert_eq!(outs.get("out").unwrap().as_str(), Some("X vs X"));
    }

    #[test]
    fn structured_data_is_serialized_as_json() {
        let gateway = Arc::new(MockGateway::new(echo));
        let env = env_of(gateway.clone());

        let outs = call(
            inputs(json!("Classify:"), json!({ "name": "ana", "age": 31 })),
            params(json!({})),
            &env,
        )
        .unwrap();
        // `serde_json` runs with `preserve_order`, so the object keeps
        // the insertion order of the blueprint's params.
        assert_eq!(
            outs.get("out").unwrap().as_str(),
            Some("Classify:\n\n{\"name\":\"ana\",\"age\":31}")
        );
    }

    #[test]
    fn placeholder_without_data_errors_instead_of_sending_a_hole() {
        let gateway = Arc::new(MockGateway::new(echo));
        let env = env_of(gateway.clone());

        let err = call(
            inputs(json!("Summarize: {{data}}"), Value::Null),
            params(json!({})),
            &env,
        )
        .unwrap_err();
        assert!(err.to_string().contains("{{data}}"));
        assert!(gateway.calls().is_empty());
    }

    #[test]
    fn array_data_broadcasts_one_call_per_element() {
        let gateway = Arc::new(MockGateway::new(echo));
        let env = env_of(gateway.clone());

        let outs = call(
            inputs(json!("Row:"), json!(["a", "b"])),
            params(json!({ "concurrency": 2 })),
            &env,
        )
        .unwrap();
        let arr = outs.get("out").unwrap().as_array().unwrap();
        assert_eq!(
            arr.iter().map(|v| v.as_str().unwrap()).collect::<Vec<_>>(),
            vec!["Row:\n\na", "Row:\n\nb"]
        );
        assert_eq!(gateway.calls().len(), 2);
    }

    #[test]
    fn equal_length_arrays_are_zipped() {
        let gateway = Arc::new(MockGateway::new(echo));
        let env = env_of(gateway.clone());

        let outs = call(
            inputs(json!(["P1", "P2"]), json!(["d1", "d2"])),
            params(json!({})),
            &env,
        )
        .unwrap();
        let arr = outs.get("out").unwrap().as_array().unwrap();
        assert_eq!(
            arr.iter().map(|v| v.as_str().unwrap()).collect::<Vec<_>>(),
            vec!["P1\n\nd1", "P2\n\nd2"]
        );
    }

    #[test]
    fn single_element_data_broadcasts_over_a_prompt_array() {
        let gateway = Arc::new(MockGateway::new(echo));
        let env = env_of(gateway.clone());

        let outs = call(
            inputs(json!(["P1", "P2"]), json!(["only"])),
            params(json!({})),
            &env,
        )
        .unwrap();
        let arr = outs.get("out").unwrap().as_array().unwrap();
        assert_eq!(
            arr.iter().map(|v| v.as_str().unwrap()).collect::<Vec<_>>(),
            vec!["P1\n\nonly", "P2\n\nonly"]
        );
    }

    #[test]
    fn mismatched_array_lengths_error() {
        let gateway = Arc::new(MockGateway::new(echo));
        let env = env_of(gateway.clone());

        let err = call(
            inputs(json!(["P1", "P2"]), json!(["d1", "d2", "d3"])),
            params(json!({})),
            &env,
        )
        .unwrap_err();
        assert!(err.to_string().contains("must have the same length"));
        assert!(gateway.calls().is_empty());
    }

    #[test]
    fn empty_data_array_yields_an_empty_output_array() {
        let gateway = Arc::new(MockGateway::new(echo));
        let env = env_of(gateway.clone());

        let outs = call(
            inputs(json!("Row:"), json!([])),
            params(json!({})),
            &env,
        )
        .unwrap();
        assert_eq!(outs.get("out").unwrap().as_array().unwrap().len(), 0);
        assert!(gateway.calls().is_empty());
    }

    #[test]
    fn blank_prompt_is_an_error_even_with_data() {
        let gateway = Arc::new(MockGateway::new(echo));
        let env = env_of(gateway.clone());

        // Data alone is not an instruction: sending it as the prompt
        // would make the node silently "work" with no question asked.
        let err = call(
            inputs(json!("   "), json!("just the data")),
            params(json!({})),
            &env,
        )
        .unwrap_err();
        assert!(err.to_string().contains("prompt is required"));
        assert!(gateway.calls().is_empty());
    }
}
