//! json.parse — parse a JSON string into a structured value.
//!
//! Pull a `String` from input `text` and return the parsed `Value`.
//! Non-string inputs are coerced to a string via the shared
//! `value_to_string` helper (e.g. `Null → "null"`, `Bool → "true"`)
//! before parsing — this lets a `literal.json` output feed straight in
//! without an intermediate `stringify → parse` round-trip.

use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::executor::{
    single, value_to_string, EvalCtx, ExecError, NodeInputs, NodeOutputs,
};

pub const KIND: &str = "json.parse";

pub fn exec(
    inputs: &NodeInputs,
    _params: &Map<String, Value>,
    _ctx: &mut EvalCtx,
) -> Result<NodeOutputs, ExecError> {
    let supplied = inputs.get("text").copied().unwrap_or(&Value::Null);

    // `Value::Null` is a valid JSON literal — short-circuit before
    // the value_to_string coerce (which maps Null → "") so we don't
    // hit the empty-input guard below.
    if supplied.is_null() {
        return Ok(single("out", Value::Null));
    }

    let text = value_to_string(&supplied);

    // Empty input is rejected explicitly so users get a clear "you
    // wired nothing into `text`" message instead of `serde_json`'s
    // cryptic "EOF while parsing a value".
    if text.is_empty() {
        return Err(ExecError::Node {
            node: KIND.into(),
            message: "input `text` is empty".into(),
        });
    }

    let parsed: Value =
        serde_json::from_str(&text).map_err(|e| ExecError::Node {
            node: KIND.into(),
            message: format!("invalid JSON: {e}"),
        })?;

    Ok(HashMap::from([("out".to_string(), parsed)]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::nodes::test_helpers::{ctx_with, empty_inputs};
    use serde_json::json;

    fn run(text: &str) -> Result<NodeOutputs, ExecError> {
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let ext = HashMap::new();
        let mut inputs = empty_inputs(&Value::Null);
        let text_value = Value::String(text.into());
        inputs.insert("text".to_string(), &text_value);
        let params = Map::new();
        let mut ctx = ctx_with("n1", KIND, &ext, &mut outs, &mut logs);
        exec(&inputs, &params, &mut ctx)
    }

    #[test]
    fn parses_object() {
        let out = run(r#"{"a": 1, "b": [true, false]}"#).expect("ok");
        assert_eq!(
            out.get("out"),
            Some(&json!({"a": 1, "b": [true, false]}))
        );
    }

    #[test]
    fn parses_array() {
        let out = run("[1, 2, 3]").expect("ok");
        assert_eq!(out.get("out"), Some(&json!([1, 2, 3])));
    }

    #[test]
    fn parses_scalar_null() {
        let out = run("null").expect("ok");
        assert_eq!(out.get("out"), Some(&Value::Null));
    }

    #[test]
    fn parses_scalar_string() {
        // JSON string literal — note the surrounding quotes in the JSON.
        let out = run(r#""hello""#).expect("ok");
        assert_eq!(out.get("out"), Some(&Value::String("hello".into())));
    }

    #[test]
    fn parses_scalar_number() {
        let out = run("3.14").expect("ok");
        match out.get("out").unwrap() {
            Value::Number(n) => assert_eq!(n.as_f64().unwrap(), 3.14),
            other => panic!("expected Number, got {other:?}"),
        }
    }

    #[test]
    fn parses_scalar_bool() {
        let out = run("true").expect("ok");
        assert_eq!(out.get("out"), Some(&Value::Bool(true)));
    }

    #[test]
    fn invalid_json_errors_with_pretty_message() {
        let err = run("{not json").unwrap_err();
        match err {
            ExecError::Node { node, message } => {
                assert_eq!(node, KIND);
                assert!(
                    message.contains("invalid JSON"),
                    "msg was: {message}"
                );
            }
            other => panic!("expected Node, got {other:?}"),
        }
    }

    #[test]
    fn empty_input_errors() {
        let err = run("").unwrap_err();
        match err {
            ExecError::Node { node, message } => {
                assert_eq!(node, KIND);
                assert!(message.contains("empty"), "msg was: {message}");
            }
            other => panic!("expected Node, got {other:?}"),
        }
    }

    #[test]
    fn coerces_non_string_input_via_value_to_string() {
        // Null → "null" → parses to Value::Null. Verifies the
        // coercion path users get when chaining a literal.json.
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let ext = HashMap::new();
        let mut inputs = empty_inputs(&Value::Null);
        let null_value = Value::Null;
        inputs.insert("text".to_string(), &null_value);
        let params = Map::new();
        let mut ctx = ctx_with("n1", KIND, &ext, &mut outs, &mut logs);
        let out = exec(&inputs, &params, &mut ctx).expect("ok");
        assert_eq!(out.get("out"), Some(&Value::Null));
    }
}
