//! `blueprint.subgraph` — run another saved blueprint as a single node.
//!
//! The editor presents the embedded blueprint's declared inputs as target
//! ports and its `io.output` names as source ports, so the graph composes
//! as if it were one node. At run time this executor:
//!
//!   1. reads the blueprint file named by `params.path`,
//!   2. passes every non-`exec`, non-null port value in as an external
//!      input (keyed by the embedded blueprint's input ids),
//!   3. runs it through the same runner (so nested AI nodes keep working),
//!   4. returns its `io.output` values as this node's outputs.
//!
//! Nesting is bounded by `runner::MAX_DEPTH`; an indirect cycle (A embeds
//! B embeds A) therefore fails with a clear message instead of recursing
//! forever.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::{Map, Value};

use crate::ai::AiEnv;
use crate::executor::runner::run_blueprint_path;
use crate::executor::{ExecError, NodeOutputs};

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
    let path = params
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if path.is_empty() {
        return Err(ExecError::Node {
            node: env.node_id.clone(),
            message: "no embedded blueprint selected: set 'Embedded blueprint' in the Inspector"
                .into(),
        });
    }

    let mut external: HashMap<String, Value> = HashMap::with_capacity(inputs.len());
    for (port, value) in inputs {
        // `then_in` is an ordering token, and Null means "not provided" so
        // the embedded blueprint's declared defaults still apply.
        if port == "then_in" || value.is_null() {
            continue;
        }
        external.insert(port.clone(), value.clone());
    }

    let result = run_blueprint_path(
        path,
        &external,
        Arc::clone(&env.gateway),
        env.depth + 1,
    )
    .await
    .map_err(|message| ExecError::Node {
        node: env.node_id.clone(),
        message,
    })?;

    let mut outs: NodeOutputs = result.outputs;
    outs.insert("then_out".into(), Value::Null);
    Ok(outs)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;

    use super::*;
    use crate::ai::mock::MockGateway;
    use crate::ai::UnconfiguredGateway;
    use crate::executor::nodes::test_helpers::unique_tmp;

    /// A tiny blueprint: `who` + "hi " → output `greeting`.
    fn write_blueprint() -> PathBuf {
        let mut path = unique_tmp("subgraph");
        path.set_extension("json");
        let bp = json!({
            "schema_version": 1,
            "id": "sub",
            "name": "Sub",
            "description": "",
            "nodes": [
                {
                    "id": "in",
                    "kind": "blueprint.input",
                    "position": {"x": 0.0, "y": 0.0},
                    "params": {},
                    "inputs": [],
                    "outputs": []
                },
                {
                    "id": "lit",
                    "kind": "literal.string",
                    "position": {"x": 0.0, "y": 0.0},
                    "params": {"value": "hi "},
                    "inputs": [],
                    "outputs": [{"id": "value", "name": "Value", "type": "string"}]
                },
                {
                    "id": "cat",
                    "kind": "string.concat",
                    "position": {"x": 0.0, "y": 0.0},
                    "params": {},
                    "inputs": [
                        {"id": "then_in", "name": "Then", "type": "exec"},
                        {"id": "a", "name": "A", "type": "string"},
                        {"id": "b", "name": "B", "type": "string"}
                    ],
                    "outputs": [
                        {"id": "then_out", "name": "Then", "type": "exec"},
                        {"id": "out", "name": "Out", "type": "string"}
                    ]
                },
                {
                    "id": "out",
                    "kind": "io.output",
                    "position": {"x": 0.0, "y": 0.0},
                    "params": {"name": "greeting"},
                    "inputs": [
                        {"id": "then_in", "name": "Then", "type": "exec"},
                        {"id": "in", "name": "In", "type": "any"}
                    ],
                    "outputs": []
                }
            ],
            "connections": [
                {"id": "e1", "sourceNode": "lit", "sourcePort": "value", "targetNode": "cat", "targetPort": "a"},
                {"id": "e2", "sourceNode": "in", "sourcePort": "who", "targetNode": "cat", "targetPort": "b"},
                {"id": "e3", "sourceNode": "cat", "sourcePort": "out", "targetNode": "out", "targetPort": "in"}
            ],
            "metadata": {
                "tags": [],
                "inputs": [{"id": "who", "name": "Who", "type": "string", "description": ""}]
            }
        });
        std::fs::write(&path, serde_json::to_string(&bp).unwrap()).unwrap();
        path
    }

    fn env_with(gateway: Arc<dyn crate::ai::AiGateway>, depth: usize) -> AiEnv {
        AiEnv {
            node_id: "sg".into(),
            gateway,
            depth,
        }
    }

    fn params_for(path: &std::path::Path) -> Map<String, Value> {
        json!({ "path": path.to_string_lossy().to_string() })
            .as_object()
            .unwrap()
            .clone()
    }

    #[test]
    fn runs_the_embedded_blueprint_and_returns_named_outputs() {
        let path = write_blueprint();
        let gateway: Arc<dyn crate::ai::AiGateway> = Arc::new(MockGateway::new(|p| p.to_string()));
        let env = env_with(gateway, 0);

        let mut inputs = Map::new();
        inputs.insert("who".into(), json!("Ada"));
        // Ordering token: must be ignored, not forwarded.
        inputs.insert("then_in".into(), Value::Null);

        let outs = tauri::async_runtime::block_on(exec(&inputs, &params_for(&path), &env))
            .expect("embedded blueprint must run");
        assert_eq!(outs.get("greeting").unwrap().as_str(), Some("hi Ada"));
        assert!(matches!(outs.get("then_out"), Some(Value::Null)));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn missing_path_errors_before_any_io() {
        let env = env_with(Arc::new(UnconfiguredGateway), 0);
        let params = Map::new();
        let err = tauri::async_runtime::block_on(exec(&Map::new(), &params, &env)).unwrap_err();
        assert!(err.to_string().contains("no embedded blueprint"), "{err}");
    }

    #[test]
    fn nesting_limit_stops_before_reading_the_file() {
        let gateway: Arc<dyn crate::ai::AiGateway> = Arc::new(MockGateway::new(|p| p.to_string()));
        let env = env_with(gateway, crate::executor::runner::MAX_DEPTH);
        let params = json!({ "path": "/does/not/exist.json" })
            .as_object()
            .unwrap()
            .clone();
        let err = tauri::async_runtime::block_on(exec(&Map::new(), &params, &env)).unwrap_err();
        assert!(err.to_string().contains("nesting exceeds"), "{err}");
    }

    #[test]
    fn unreadable_file_reports_the_path() {
        let env = env_with(Arc::new(UnconfiguredGateway), 0);
        let params = json!({ "path": "/does/not/exist.json" })
            .as_object()
            .unwrap()
            .clone();
        let err = tauri::async_runtime::block_on(exec(&Map::new(), &params, &env)).unwrap_err();
        assert!(err.to_string().contains("/does/not/exist.json"), "{err}");
    }
}
