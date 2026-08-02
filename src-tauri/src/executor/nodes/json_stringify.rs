//! json.stringify — serialize a value to a JSON string.
//!
//! `params.pretty` toggles between compact (`serde_json::to_string`)
//! and indented (`serde_json::to_string_pretty`). Any value goes in;
//! a string goes out. Non-string inputs coerce via `value_to_string`
//! upstream — but stringify itself accepts any JSON value type.

use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::executor::{EvalCtx, ExecError, NodeInputs, NodeOutputs};

pub const KIND: &str = "json.stringify";

pub fn exec(
    inputs: &NodeInputs,
    params: &Map<String, Value>,
    _ctx: &mut EvalCtx,
) -> Result<NodeOutputs, ExecError> {
    let supplied = inputs.get("value").copied().unwrap_or(&Value::Null);
    let pretty = matches!(params.get("pretty"), Some(Value::Bool(true)));

    let serialized = if pretty {
        serde_json::to_string_pretty(&supplied)
    } else {
        serde_json::to_string(&supplied)
    }
    .map_err(|e| ExecError::Node {
        node: KIND.into(),
        message: format!("cannot serialize value: {e}"),
    })?;

    Ok(HashMap::from([(
        "out".to_string(),
        Value::String(serialized),
    )]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::nodes::test_helpers::{ctx_with, empty_inputs};
    use serde_json::json;

    fn run(value: Value, pretty: Option<bool>) -> Result<NodeOutputs, ExecError> {
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let ext = HashMap::new();
        let mut inputs = empty_inputs(&Value::Null);
        inputs.insert("value".to_string(), &value);
        let mut params = Map::new();
        if let Some(p) = pretty {
            params.insert("pretty".to_string(), Value::Bool(p));
        }
        let mut ctx = ctx_with("n1", KIND, &ext, &mut outs, &mut logs);
        exec(&inputs, &params, &mut ctx)
    }

    #[test]
    fn compact_object_is_single_line() {
        let out = run(json!({"a": 1, "b": 2}), Some(false)).expect("ok");
        let s = out.get("out").unwrap().as_str().unwrap();
        assert!(!s.contains('\n'), "expected compact, got: {s}");
        assert_eq!(s, r#"{"a":1,"b":2}"#);
    }

    #[test]
    fn pretty_object_indents() {
        let out = run(json!({"a": 1, "b": 2}), Some(true)).expect("ok");
        let s = out.get("out").unwrap().as_str().unwrap();
        assert!(s.contains('\n'), "expected pretty, got: {s}");
        assert!(s.starts_with('{'));
        assert!(s.trim_end().ends_with('}'));
    }

    #[test]
    fn arrays_serialize_correctly() {
        let out = run(json!([1, 2, 3]), Some(false)).expect("ok");
        assert_eq!(out.get("out").unwrap().as_str().unwrap(), "[1,2,3]");
    }

    #[test]
    fn null_input_yields_null_string() {
        let out = run(Value::Null, Some(false)).expect("ok");
        assert_eq!(out.get("out").unwrap().as_str().unwrap(), "null");
    }

    #[test]
    fn bool_input_compact() {
        let out = run(Value::Bool(true), Some(false)).expect("ok");
        assert_eq!(out.get("out").unwrap().as_str().unwrap(), "true");
    }

    #[test]
    fn pretty_param_defaults_to_false_when_missing() {
        let out = run(json!({"a": 1}), None).expect("ok");
        let s = out.get("out").unwrap().as_str().unwrap();
        assert_eq!(s, r#"{"a":1}"#);
    }
}
