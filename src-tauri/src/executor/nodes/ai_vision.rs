//! `ai.vision` — send images (plus a prompt) to a vision-capable model.
//!
//! Ports:
//!   * `then_in` / `then_out` — ordering.
//!   * `prompt` — instruction; a wired value wins over the body default.
//!   * `image_1 … image_N` — N runtime slots (`repeatableInputs`). Each
//!     slot accepts either one base64 image (the `binary` convention used
//!     by `file.read`) or an **array** of images, so the total number of
//!     images in one request is not bounded by the slot count.
//!
//! The prompt mirrors the rest of the catalog: a scalar sends one request,
//! an array sends one request per element with the same images. Sockets
//! that are unwired arrive as `Null` and are skipped.
//!
//! Images are validated as base64 here so a bad wire fails before a
//! provider round-trip. The MIME type is sniffed from the payload in
//! `ai::providers` (the node only carries the data).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use futures::stream::{self, StreamExt, TryStreamExt};
use serde_json::{Map, Value};

use crate::ai::{AiEnv, AiRequest};
use crate::executor::nodes::ai_completion::resolve_target;
use crate::executor::{decode_b64, single, value_to_string, ExecError, NodeOutputs};

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
    let providers = env.gateway.providers().map_err(|e| ExecError::Node {
        node: env.node_id.clone(),
        message: e.to_string(),
    })?;
    let target = resolve_target(params, &providers, &env.node_id)?;

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

    let images = collect_images(inputs, &env.node_id)?;
    if images.is_empty() {
        return Err(ExecError::Node {
            node: env.node_id.clone(),
            message: "connect at least one image (the `image_N` slots accept a base64 image or an array of them)".into(),
        });
    }

    let (prompts, mirror) = prompts_of(
        inputs.get("prompt").unwrap_or(&Value::Null),
        &env.node_id,
    )?;

    let requests: Vec<AiRequest> = prompts
        .into_iter()
        .map(|prompt| AiRequest {
            provider_id: target.provider_id.clone(),
            provider: target.provider,
            model: target.model.clone(),
            base_url: target.base_url.clone(),
            prompt,
            system: system.clone(),
            temperature,
            max_tokens,
            images: images.clone(),
        })
        .collect();

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

    if mirror {
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

/// Flatten every image slot in port order. `Null` / blank slots are
/// skipped; arrays contribute one image per non-blank element.
fn collect_images(
    inputs: &Map<String, Value>,
    node: &str,
) -> Result<Vec<String>, ExecError> {
    // `inputs` is a HashMap, so iterate the slot names in numeric order:
    // image_2 must come before image_10, and iteration order must never
    // decide which image the model sees first.
    let mut ports: Vec<&String> = inputs
        .keys()
        .filter(|port| port.as_str() != "then_in" && port.as_str() != "prompt")
        .collect();
    ports.sort_by_key(|port| slot_index(port));

    let mut images: Vec<String> = Vec::new();
    for port in ports {
        let value = &inputs[port];
        match value {
            Value::Null => {}
            Value::String(text) if text.trim().is_empty() => {}
            Value::String(text) => push_image(&mut images, text, port.as_str(), node)?,
            Value::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    match item {
                        Value::Null => {}
                        Value::String(text) if text.trim().is_empty() => {}
                        Value::String(text) => push_image(
                            &mut images,
                            text,
                            &format!("{port}[{i}]"),
                            node,
                        )?,
                        other => {
                            return Err(ExecError::Node {
                                node: node.to_string(),
                                message: format!(
                                    "{port}[{i}] must be a base64 string, got {}",
                                    crate::executor::value_type_name(other)
                                ),
                            })
                        }
                    }
                }
            }
            other => {
                return Err(ExecError::Node {
                    node: node.to_string(),
                    message: format!(
                        "{port} must be a base64 image or an array of them, got {}",
                        crate::executor::value_type_name(other)
                    ),
                })
            }
        }
    }
    Ok(images)
}

/// Sort key for a repeatable slot id: `image_3` → 3. A name without a
/// numeric suffix sorts last rather than panicking.
fn slot_index(port: &str) -> usize {
    port.rsplit('_')
        .next()
        .and_then(|n| n.parse::<usize>().ok())
        .unwrap_or(usize::MAX)
}

fn push_image(
    images: &mut Vec<String>,
    raw: &str,
    port: &str,
    node: &str,
) -> Result<(), ExecError> {
    let data = raw.trim();
    decode_b64(data).map_err(|e| ExecError::Node {
        node: node.to_string(),
        message: format!("{port} is not valid base64: {e}"),
    })?;
    images.push(data.to_string());
    Ok(())
}

/// Resolve the prompt port into 1..N prompts plus whether the output must
/// be an array.
fn prompts_of(value: &Value, node: &str) -> Result<(Vec<String>, bool), ExecError> {
    match value {
        Value::String(text) if !text.trim().is_empty() => Ok((vec![text.clone()], false)),
        Value::Array(items) if !items.is_empty() => {
            Ok((items.iter().map(value_to_string).collect(), true))
        }
        _ => Err(ExecError::Node {
            node: node.to_string(),
            message:
                "prompt is required: wire the 'Prompt' port or set the 'Prompt (default)' param"
                    .into(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::*;
    use crate::ai::mock::MockGateway;

    fn env_of(gateway: Arc<MockGateway>) -> AiEnv {
        AiEnv {
            node_id: "vision1".into(),
            gateway,
            depth: 0,
        }
    }

    fn params(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    fn call(
        inputs: Map<String, Value>,
        params: Map<String, Value>,
        env: &AiEnv,
    ) -> Result<std::collections::HashMap<String, Value>, ExecError> {
        tauri::async_runtime::block_on(exec(&inputs, &params, env))
    }

    const PNG: &str = "iVBORw0KGgoAAAANSUhEUg==";
    const JPEG: &str = "/9j/4AAQSkZJRg==";

    #[test]
    fn sends_every_image_in_port_order() {
        let gateway = Arc::new(MockGateway::new(|p| p.to_uppercase()));
        let env = env_of(gateway.clone());
        let mut inputs = Map::new();
        inputs.insert("prompt".into(), json!("describe"));
        inputs.insert("image_1".into(), json!(PNG));
        inputs.insert("image_2".into(), json!(JPEG));

        let outs = call(inputs, params(json!({})), &env).unwrap();
        assert_eq!(outs.get("out").unwrap().as_str(), Some("DESCRIBE"));
        let calls = gateway.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].images, vec![PNG.to_string(), JPEG.to_string()]);
    }

    #[test]
    fn slots_are_sorted_numerically_not_lexicographically() {
        let gateway = Arc::new(MockGateway::new(|p| p.to_string()));
        let env = env_of(gateway.clone());
        let mut inputs = Map::new();
        inputs.insert("prompt".into(), json!("describe"));
        // HashMap order is arbitrary; `image_2` must still precede `image_10`.
        inputs.insert("image_10".into(), json!(JPEG));
        inputs.insert("image_2".into(), json!(PNG));

        call(inputs, params(json!({})), &env).unwrap();
        assert_eq!(
            gateway.calls()[0].images,
            vec![PNG.to_string(), JPEG.to_string()]
        );
    }

    #[test]
    fn an_array_in_one_slot_counts_as_many_images() {
        let gateway = Arc::new(MockGateway::new(|p| p.to_string()));
        let env = env_of(gateway.clone());
        let mut inputs = Map::new();
        inputs.insert("prompt".into(), json!("compare"));
        // `file.read` on several files emits exactly this shape.
        inputs.insert("image_1".into(), json!([PNG, JPEG, Value::Null]));
        inputs.insert("image_2".into(), Value::Null);

        call(inputs, params(json!({})), &env).unwrap();
        assert_eq!(gateway.calls()[0].images.len(), 2);
    }

    #[test]
    fn prompt_array_mirrors_the_output_shape() {
        let gateway = Arc::new(MockGateway::new(|p| p.to_string()));
        let env = env_of(gateway.clone());
        let mut inputs = Map::new();
        inputs.insert("prompt".into(), json!(["one", "two"]));
        inputs.insert("image_1".into(), json!(PNG));

        let outs = call(inputs, params(json!({})), &env).unwrap();
        let arr = outs.get("out").unwrap().as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(gateway.calls().len(), 2);
        // Images are broadcast to every prompt call.
        assert_eq!(gateway.calls()[1].images, vec![PNG.to_string()]);
    }

    #[test]
    fn no_images_errors_before_calling_out() {
        let gateway = Arc::new(MockGateway::new(|p| p.to_string()));
        let env = env_of(gateway.clone());
        let mut inputs = Map::new();
        inputs.insert("prompt".into(), json!("describe"));
        inputs.insert("image_1".into(), Value::Null);

        let err = call(inputs, params(json!({})), &env).unwrap_err();
        assert!(err.to_string().contains("at least one image"), "{err}");
        assert!(gateway.calls().is_empty());
    }

    #[test]
    fn invalid_base64_names_the_slot() {
        let gateway = Arc::new(MockGateway::new(|p| p.to_string()));
        let env = env_of(gateway);
        let mut inputs = Map::new();
        inputs.insert("prompt".into(), json!("describe"));
        inputs.insert("image_1".into(), json!("not base64!!"));

        let err = call(inputs, params(json!({})), &env).unwrap_err();
        assert!(err.to_string().contains("image_1"), "{err}");
        assert!(err.to_string().contains("base64"), "{err}");
    }

    #[test]
    fn non_string_image_errors() {
        let gateway = Arc::new(MockGateway::new(|p| p.to_string()));
        let env = env_of(gateway);
        let mut inputs = Map::new();
        inputs.insert("prompt".into(), json!("describe"));
        inputs.insert("image_1".into(), json!(42));

        let err = call(inputs, params(json!({})), &env).unwrap_err();
        assert!(err.to_string().contains("base64 image"), "{err}");
    }
}
