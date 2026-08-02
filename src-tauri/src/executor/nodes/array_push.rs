//! `array.push` — appends `item` to `in`; returns the new array.
//! Errors with `TypeMismatch` if `in` is not an array.

use serde_json::{Map, Value};

use crate::executor::{single, value_type_name, EvalCtx, ExecError, NodeInputs, NodeOutputs};

pub fn exec(
    inputs: &NodeInputs<'_>,
    _params: &Map<String, Value>,
    _ctx: &mut EvalCtx<'_>,
) -> Result<NodeOutputs, ExecError> {
    let null = Value::Null;
    let arr = inputs.get("in").copied().unwrap_or(&null);
    let item = inputs.get("item").copied().unwrap_or(&null).clone();
    let v = match arr {
        Value::Array(xs) => {
            let mut out: Vec<Value> = Vec::with_capacity(xs.len() + 1);
            out.extend(xs.iter().cloned());
            out.push(item);
            Value::Array(out)
        }
        other => {
            return Err(ExecError::TypeMismatch {
                node: "array.push".into(),
                expected: "array".into(),
                actual: value_type_name(other).into(),
            });
        }
    };
    Ok(single("out", v))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::{json, Value};

    use super::exec;
    use super::ExecError;
    use crate::executor::nodes::test_helpers::ctx_with;

    #[test]
    fn appends_to_array() {
        let params = json!({}).as_object().unwrap().clone();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let in_val = Value::Array(vec![json!(1), json!(2)]);
        let item_val = Value::from(3);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), &in_val);
        inputs.insert("item".to_string(), &item_val);
        let mut ctx = ctx_with("n1", "array.push", &external, &mut outs, &mut logs);
        let r = exec(&inputs, &params, &mut ctx).unwrap();
        let arr = r.get("out").unwrap().as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0].as_i64(), Some(1));
        assert_eq!(arr[1].as_i64(), Some(2));
        assert_eq!(arr[2].as_i64(), Some(3));
    }

    #[test]
    fn appends_string_item() {
        let params = json!({}).as_object().unwrap().clone();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let in_val = Value::Array(vec![json!("a"), json!("b")]);
        let item_val = Value::String("c".into());
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), &in_val);
        inputs.insert("item".to_string(), &item_val);
        let mut ctx = ctx_with("n1", "array.push", &external, &mut outs, &mut logs);
        let r = exec(&inputs, &params, &mut ctx).unwrap();
        let arr = r.get("out").unwrap().as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[2].as_str(), Some("c"));
    }

    #[test]
    fn errors_on_non_array_input() {
        let params = json!({}).as_object().unwrap().clone();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let in_val = Value::from(5);
        let item_val = Value::from(1);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), &in_val);
        inputs.insert("item".to_string(), &item_val);
        let mut ctx = ctx_with("n1", "array.push", &external, &mut outs, &mut logs);
        let err = exec(&inputs, &params, &mut ctx).unwrap_err();
        assert!(matches!(err, ExecError::TypeMismatch { .. }));
    }
}
