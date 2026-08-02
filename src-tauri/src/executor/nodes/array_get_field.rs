//! array.getField — extract one value from each item in an array.
//!
//! `params.field` is interpreted based on the element's runtime type:
//!
//! - Objects: `field` is a property name; lookup via `map.get(field)`.
//! - Arrays:  `field` is parsed as a `u64` and used as an index
//!   (`inner.get(idx)`). Lets you pull a single column out of a
//!   "table"-shaped JSON encoded as `Vec<Vec<T>>`.
//!
//! Failure modes:
//!
//! - `params.skipInvalid = false` (default): emit a `TypeMismatch`
//!   error. Loud but unambiguous — the runtime tells you exactly
//!   which row tripped it and why (missing key, out-of-range index,
//!   non-numeric `field` on an array element, or non-object/non-array
//!   row entirely).
//! - `params.skipInvalid = true`: drop those rows silently. The
//!   output array may be shorter than the input.

use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::executor::{value_type_name, EvalCtx, ExecError, NodeInputs, NodeOutputs};

pub const KIND: &str = "array.getField";

pub fn exec(
    inputs: &NodeInputs,
    params: &Map<String, Value>,
    _ctx: &mut EvalCtx,
) -> Result<NodeOutputs, ExecError> {
    let arr_input = inputs.get("in").copied().unwrap_or(&Value::Null);
    let arr = match &arr_input {
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

    let skip_invalid = matches!(params.get("skipInvalid"), Some(Value::Bool(true)));
    let mut out: Vec<Value> = Vec::with_capacity(arr.len());
    for (idx, el) in arr.iter().enumerate() {
        match el {
            Value::Object(map) => match map.get(&field) {
                Some(v) => out.push(v.clone()),
                None => {
                    if !skip_invalid {
                        return Err(ExecError::TypeMismatch {
                            node: KIND.into(),
                            expected: format!("object with field `{field}`"),
                            actual: format!("missing field `{field}` at index {idx}"),
                        });
                    }
                }
            },
            Value::Array(inner) => {
                // Inside an array-of-arrays, `params.field` plays the
                // role of an index — the same string is a property
                // name on object elements and a numeric index on array
                // elements, decided by the element's runtime type.
                match field.parse::<u64>() {
                    Ok(i) => match inner.get(i as usize) {
                        Some(v) => out.push(v.clone()),
                        None => {
                            if !skip_invalid {
                                return Err(ExecError::TypeMismatch {
                                    node: KIND.into(),
                                    expected: format!(
                                        "array indexable in range 0..{}",
                                        inner.len()
                                    ),
                                    actual: format!(
                                        "no element at index `{field}` at row {idx} (len {})",
                                        inner.len()
                                    ),
                                });
                            }
                        }
                    },
                    Err(_) => {
                        if !skip_invalid {
                            return Err(ExecError::TypeMismatch {
                                node: KIND.into(),
                                expected: "numeric `field` for array elements".into(),
                                actual: format!(
                                    "non-numeric field `{field}` at row {idx} (element is array, not object)"
                                ),
                            });
                        }
                    }
                }
            }
            _ => {
                if !skip_invalid {
                    return Err(ExecError::TypeMismatch {
                        node: KIND.into(),
                        expected: "object or array".into(),
                        actual: format!("{} at index {}", value_type_name(el), idx),
                    });
                }
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

    fn run(arr: Value, field: &str, skip: Option<bool>) -> Result<NodeOutputs, ExecError> {
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let ext = HashMap::new();
        let mut inputs = empty_inputs(&Value::Null);
        inputs.insert("in".to_string(), &arr);
        let mut params = Map::new();
        params.insert("field".to_string(), Value::String(field.into()));
        if let Some(s) = skip {
            params.insert("skipInvalid".to_string(), Value::Bool(s));
        }
        let mut ctx = ctx_with("n1", KIND, &ext, &mut outs, &mut logs);
        exec(&inputs, &params, &mut ctx)
    }

    #[test]
    fn extracts_field_from_each_object() {
        let arr = json!([{"name": "a"}, {"name": "b"}, {"name": "c"}]);
        let out = run(arr, "name", None).expect("ok");
        assert_eq!(out.get("out"), Some(&json!(["a", "b", "c"])));
    }

    #[test]
    fn loud_missing_field_errors() {
        let arr = json!([{"name": "a"}, {"id": 1}]);
        // `{"id": 1}` is at index 1 — it's missing the `name` field.
        let err = run(arr, "name", Some(false)).unwrap_err();
        match &err {
            ExecError::TypeMismatch { node, .. } => assert_eq!(node, KIND),
            other => panic!("expected TypeMismatch, got {other:?}"),
        }
        // Index of the offending element must appear in the error.
        assert!(
            err.to_string().contains("at index 1"),
            "expected idx in error, got: {err}"
        );
    }

    #[test]
    fn skip_missing_field_keeps_clean_only() {
        let arr = json!([{"name": "a"}, {"id": 1}, {"name": "b"}]);
        let out = run(arr, "name", Some(true)).expect("ok");
        assert_eq!(out.get("out"), Some(&json!(["a", "b"])));
    }

    #[test]
    fn loud_non_object_errors() {
        let arr = json!([{"name": "a"}, "not-an-object"]);
        // "not-an-object" is at index 1.
        let err = run(arr, "name", Some(false)).unwrap_err();
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
    fn skip_non_object_keeps_clean_only() {
        let arr = json!([{"name": "a"}, 42, [1, 2], {"name": "b"}]);
        let out = run(arr, "name", Some(true)).expect("ok");
        assert_eq!(out.get("out"), Some(&json!(["a", "b"])));
    }

    #[test]
    fn empty_array_yields_empty_array() {
        let arr = json!([]);
        let out = run(arr, "name", None).expect("ok");
        assert_eq!(out.get("out"), Some(&json!([])));
    }

    #[test]
    fn missing_field_errors_by_default() {
        // skipInvalid param omitted entirely — defaults to loud.
        let arr = json!([{"name": "a"}, {"id": 1}]);
        let err = run(arr, "name", None).unwrap_err();
        assert!(matches!(err, ExecError::TypeMismatch { .. }));
    }

    #[test]
    fn empty_field_errors() {
        let err = run(json!([{"a": 1}]), "", None).unwrap_err();
        match err {
            ExecError::Node { node, message } => {
                assert_eq!(node, KIND);
                assert!(message.contains("empty"), "msg: {message}");
            }
            other => panic!("expected Node, got {other:?}"),
        }
    }

    #[test]
    fn wrong_input_type_errors() {
        let err = run(json!({"not": "array"}), "x", None).unwrap_err();
        match err {
            ExecError::TypeMismatch { node, .. } => assert_eq!(node, KIND),
            other => panic!("expected TypeMismatch, got {other:?}"),
        }
    }

    // ---- array-of-arrays (column-extraction) tests ----

    #[test]
    fn extracts_index_from_array_elements() {
        let arr = json!([[10, 20, 30], [40, 50, 60], [70, 80, 90]]);
        let out = run(arr, "1", None).expect("ok");
        assert_eq!(out.get("out"), Some(&json!([20, 50, 80])));
    }

    #[test]
    fn extracts_first_column_from_table_shape() {
        // Canonical CSV-shaped blob as a 2-D array.
        let arr = json!([["a", "b", "c"], ["d", "e", "f"], ["g", "h", "i"]]);
        let out = run(arr, "0", None).expect("ok");
        assert_eq!(out.get("out"), Some(&json!(["a", "d", "g"])));
    }

    #[test]
    fn out_of_range_array_index_errors() {
        let arr = json!([[1, 2, 3], [4, 5, 6]]);
        let err = run(arr, "5", Some(false)).unwrap_err();
        match &err {
            ExecError::TypeMismatch { node, actual, .. } => {
                assert_eq!(node, KIND);
                assert!(actual.contains("at row 0"), "msg: {actual}");
            }
            other => panic!("expected TypeMismatch, got {other:?}"),
        }
    }

    #[test]
    fn non_numeric_field_on_array_element_errors() {
        let arr = json!([[1, 2, 3]]);
        let err = run(arr, "first", Some(false)).unwrap_err();
        match &err {
            ExecError::TypeMismatch { node, actual, .. } => {
                assert_eq!(node, KIND);
                assert!(actual.contains("non-numeric"), "msg: {actual}");
            }
            other => panic!("expected TypeMismatch, got {other:?}"),
        }
    }

    #[test]
    fn mixed_object_and_array_rows_skip_drops_unparseable() {
        // `name` works on the object rows; on array rows it isn't a
        // valid u64 and gets dropped because skipInvalid=true.
        let arr = json!([
            {"name": "alpha"},
            [100, 200, 300],
            {"name": "beta"},
            [400, 500]
        ]);
        let out = run(arr, "name", Some(true)).expect("ok");
        assert_eq!(out.get("out"), Some(&json!(["alpha", "beta"])));
    }
}
