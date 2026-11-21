//! `ai.tool` — expose a saved blueprint to an AI node as a callable tool.
//!
//! The second tool flavour: a node, not global configuration. Its `Tool`
//! output carries a plain descriptor (name, description, JSON Schema and
//! the blueprint path). Wire it into an `ai.completion` node's `Tools`
//! port (bundle several through `array.create`) and the model can ask for
//! the tool; the AI node then runs the referenced blueprint with the
//! tool-call arguments as its external inputs.
//!
//! Nothing secret lives here: a blueprint path and a prompt-facing
//! description are safe to save and share.

use serde_json::{json, Map, Value};

use crate::executor::{single, EvalCtx, ExecError, NodeInputs, NodeOutputs};

/// Default JSON Schema used when the user leaves `parameters` blank.
fn default_parameters() -> Value {
    json!({ "type": "object", "properties": {} })
}

pub fn exec(
    _inputs: &NodeInputs<'_>,
    params: &Map<String, Value>,
    ctx: &mut EvalCtx<'_>,
) -> Result<NodeOutputs, ExecError> {
    let path = params
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if path.is_empty() {
        return Err(ExecError::Node {
            node: ctx.node_id.to_string(),
            message: "no tool blueprint selected: set 'Tool blueprint' in the Inspector".into(),
        });
    }

    // Fall back to the file name so a freshly dropped node still has a
    // usable function name.
    let name_raw = params
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    let name = if name_raw.is_empty() {
        crate::app_tools::sanitize_tool_name(file_stem(path))
    } else {
        crate::app_tools::sanitize_tool_name(name_raw)
    };
    if name.is_empty() {
        return Err(ExecError::Node {
            node: ctx.node_id.to_string(),
            message: "tool name is required (set it in the Inspector)".into(),
        });
    }

    let description = params
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("Run the '{name}' blueprint."));

    let parameters = match params.get("parameters").and_then(Value::as_str) {
        Some(raw) if !raw.trim().is_empty() => {
            serde_json::from_str::<Value>(raw).map_err(|e| ExecError::Node {
                node: ctx.node_id.to_string(),
                message: format!("tool parameters must be valid JSON Schema: {e}"),
            })?
        }
        _ => default_parameters(),
    };
    if !parameters.is_object() {
        return Err(ExecError::Node {
            node: ctx.node_id.to_string(),
            message: "tool parameters must be a JSON object (a JSON Schema)".into(),
        });
    }

    Ok(single(
        "tool",
        json!({
            "name": name,
            "description": description,
            "parameters": parameters,
            "blueprint_path": path,
        }),
    ))
}

/// `…/find-customer.blueprint.json` → `find-customer`.
fn file_stem(path: &str) -> &str {
    let file = path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path);
    file.strip_suffix(".blueprint.json")
        .or_else(|| file.strip_suffix(".json"))
        .unwrap_or(file)
}

#[cfg(test)]
mod tests {
    use crate::executor::nodes::test_helpers::{ctx_with, empty_inputs};

    use super::*;

    #[test]
    fn descriptor_carries_the_blueprint_path() {
        let params = json!({
            "path": "/tmp/find-customer.blueprint.json",
            "name": "",
            "description": "",
            "parameters": "{\"type\":\"object\",\"properties\":{\"email\":{\"type\":\"string\"}}}",
        });
        let params = params.as_object().unwrap().clone();
        let external = std::collections::HashMap::new();
        let mut outputs = std::collections::HashMap::new();
        let mut logs = Vec::new();
        let null = Value::Null;
        let mut ctx = ctx_with("tool1", "ai.tool", &external, &mut outputs, &mut logs);
        let outs = exec(&empty_inputs(&null), &params, &mut ctx).unwrap();
        let tool = outs.get("tool").unwrap();
        assert_eq!(tool["name"], "find-customer");
        assert_eq!(tool["blueprint_path"], "/tmp/find-customer.blueprint.json");
        assert_eq!(tool["parameters"]["properties"]["email"]["type"], "string");
        assert_eq!(tool["description"], "Run the 'find-customer' blueprint.");
    }

    #[test]
    fn missing_path_errors() {
        let params = Map::new();
        let external = std::collections::HashMap::new();
        let mut outputs = std::collections::HashMap::new();
        let mut logs = Vec::new();
        let null = Value::Null;
        let mut ctx = ctx_with("tool1", "ai.tool", &external, &mut outputs, &mut logs);
        let err = exec(&empty_inputs(&null), &params, &mut ctx).unwrap_err();
        assert!(err.to_string().contains("no tool blueprint"), "{err}");
    }

    #[test]
    fn invalid_parameters_json_errors() {
        let params = json!({ "path": "/tmp/x.json", "name": "x", "parameters": "{not json" });
        let params = params.as_object().unwrap().clone();
        let external = std::collections::HashMap::new();
        let mut outputs = std::collections::HashMap::new();
        let mut logs = Vec::new();
        let null = Value::Null;
        let mut ctx = ctx_with("tool1", "ai.tool", &external, &mut outputs, &mut logs);
        let err = exec(&empty_inputs(&null), &params, &mut ctx).unwrap_err();
        assert!(err.to_string().contains("JSON Schema"), "{err}");
    }
}
