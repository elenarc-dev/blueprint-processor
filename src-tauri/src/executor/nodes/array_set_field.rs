//! array.setField — replace the value of `params.field` on every
//! object in `in` with the connected `value`.
//!
//! SET semantics: when the field is missing on an object it is
//! CREATED. This matches typical "set" expectations (think a database
//! `UPDATE … SET col = $1` — updates existing, creates if absent).
//!
//! `params.skipInvalid` controls what happens with non-object elements:
//!
//! - `false` (default): any non-object element trips a `TypeMismatch`
//!   error so mismatched data fails the run loudly.
//! - `true`: non-object elements are kept untouched; only objects
//!   are updated.

use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::executor::{value_type_name, EvalCtx, ExecError, NodeInputs, NodeOutputs};

pub const KIND: &str = "array.setField";

pub fn exec(
    inputs: &NodeInputs,
    params: &Map<String, Value>,
    _ctx: &mut EvalCtx,
) -> Result<NodeOutputs, ExecError> {
    let arr_input = inputs.get("in").copied().unwrap_or(&Value::Null);        let arr = match &arr_input {
            Value::Array(a) => a,
            _ => {
                return Err(ExecError::TypeMismatch {
                    node: KIND.into(),
                    expected: "array".into(),
                    actual: value_type_name(&arr_input).into(),
                })
            }
        };

    let field = params
        .get("field")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if field.is_empty() {
        return Err(ExecError::Node {
            node: KIND.into(),
            message: "params.field is empty".into(),
        });
    }

    let skip_invalid = matches!(
        params.get("skipInvalid"),
        Some(Value::Bool(true))
    );

    let new_value = inputs.get("value").copied().unwrap_or(&Value::Null);

    let mut out: Vec<Value> = Vec::with_capacity(arr.len());
    for (idx, el) in arr.iter().enumerate() {
        match el {
            Value::Object(map) => {
                let mut m: Map<String, Value> = map.clone();
                m.insert(field.clone(), new_value.clone());
                out.push(Value::Object(m));
            }
            _ => {
                if !skip_invalid {
                    return Err(ExecError::TypeMismatch {
                        node: KIND.into(),
                        expected: "object".into(),
                        actual: format!(
                            "{} at index {}",
                            value_type_name(el),
                            idx
                        ),
                    });
                }
                out.push(el.clone());
            }
        }
    }

    Ok(HashMap::from([("out".to_string(), Value::Array(out))]))
}



#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::nodes::test_helpers::{ctx_with, empty_inputs};
    use serde_json::json;

    fn run(
        arr: Value,
        field: &str,
        value: Value,
        skip: Option<bool>,
    ) -> Result<NodeOutputs, ExecError> {
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let ext = HashMap::new();
        let mut inputs = empty_inputs(&Value::Null);
        inputs.insert("in".to_string(), &arr);
        inputs.insert("value".to_string(), &value);
        let mut params = Map::new();
        params.insert("field".to_string(), Value::String(field.into()));
        if let Some(s) = skip {
            params.insert("skipInvalid".to_string(), Value::Bool(s));
        }
        let mut ctx = ctx_with("n1", KIND, &ext, &mut outs, &mut logs);
        exec(&inputs, &params, &mut ctx)
    }

    #[test]
    fn replaces_field_on_each_object() {
        let arr = json!([{"name": "a", "x": 1}, {"name": "b", "x": 2}]);
        let out = run(arr, "name", json!("z"), None).expect("ok");
        assert_eq!(
            out.get("out"),
            Some(&json!([{"name": "z", "x": 1}, {"name": "z", "x": 2}]))
        );
    }

    #[test]
    fn creates_missing_field_set_semantics() {
        let arr = json!([{"name": "a"}, {"id": 1}]);
        let out = run(arr, "tag", json!("X"), None).expect("ok");
        assert_eq!(
            out.get("out"),
            Some(&json!([
                {"name": "a", "tag": "X"},
                {"id": 1, "tag": "X"}
            ]))
        );
    }

    #[test]
    fn loud_non_object_errors() {
        let arr = json!([{"a": 1}, 42]);
        // 42 is at index 1 — it's not an object.
        let err = run(arr, "a", json!(9), Some(false)).unwrap_err();
        match &err {
            ExecError::TypeMismatch { node, .. } => assert_eq!(node, KIND),
            other => panic!("expected TypeMismatch, got {other:?}"),
        }
        assert!(
            err.to_string().contains("at index 1"),
            "expected idx in error, got: {err}"
        );
    }

    #[test]
    fn skip_non_object_keeps_as_is() {
        let arr = json!([{"a": 1}, 42, [1, 2], {"a": 3}]);
        let out = run(arr, "a", json!(9), Some(true)).expect("ok");
        assert_eq!(
            out.get("out"),
            Some(&json!([{"a": 9}, 42, [1, 2], {"a": 9}]))
        );
    }

    #[test]
    fn empty_array_yields_empty_array() {
        let out = run(json!([]), "a", json!(1), None).expect("ok");
        assert_eq!(out.get("out"), Some(&json!([])));
    }

    #[test]
    fn null_value_clears_field() {
        let arr = json!([{"name": "a"}]);
        let out = run(arr, "name", Value::Null, None).expect("ok");
        assert_eq!(out.get("out"), Some(&json!([{"name": null}])));
    }

    #[test]
    fn wrong_input_type_errors() {
        let err = run(json!({"not": "array"}), "x", json!(1), None).unwrap_err();
        match err {
            ExecError::TypeMismatch { node, .. } => assert_eq!(node, KIND),
            other => panic!("expected TypeMismatch, got {other:?}"),
        }
    }

    #[test]
    fn empty_field_errors() {
        let err = run(json!([{"a": 1}]), "", json!(1), None).unwrap_err();
        match err {
            ExecError::Node { node, message } => {
                assert_eq!(node, KIND);
                assert!(message.contains("empty"), "msg: {message}");
            }
            other => panic!("expected Node, got {other:?}"),
        }
    }
}
