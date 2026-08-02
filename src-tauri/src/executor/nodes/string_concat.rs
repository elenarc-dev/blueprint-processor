//! `string.concat` — joins two strings with weak value-to-string coercion.
//! Broadcasts element-wise over arrays.

use serde_json::{Map, Value};

use crate::executor::broadcast::broadcast_pairs;
use crate::executor::{single, value_to_string, EvalCtx, ExecError, NodeInputs, NodeOutputs};

pub fn exec(
    inputs: &NodeInputs<'_>,
    _params: &Map<String, Value>,
    _ctx: &mut EvalCtx<'_>,
) -> Result<NodeOutputs, ExecError> {
    let null = Value::Null;
    let a = inputs.get("a").copied().unwrap_or(&null);
    let b = inputs.get("b").copied().unwrap_or(&null);
    let v = broadcast_pairs(a, b, |x, y| {
        Ok(Value::String(value_to_string(x) + &value_to_string(y)))
    })?;
    Ok(single("out", v))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::{json, Value};

    use super::exec;
    use crate::executor::nodes::test_helpers::ctx_with;

    fn run(a: Value, b: Value) -> HashMap<String, Value> {
        let params = json!({}).as_object().unwrap().clone();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let a_val = a;
        let b_val = b;
        let mut inputs = HashMap::new();
        inputs.insert("a".to_string(), &a_val);
        inputs.insert("b".to_string(), &b_val);
        let mut ctx = ctx_with("n1", "string.concat", &external, &mut outs, &mut logs);
        exec(&inputs, &params, &mut ctx).unwrap()
    }

    #[test]
    fn weak_coercion_on_numbers() {
        let r = run(Value::String("prefix: ".into()), Value::from(42));
        assert_eq!(r.get("out").unwrap().as_str(), Some("prefix: 42"));
    }

    #[test]
    fn appends_when_array_on_a() {
        let r = run(
            Value::Array(vec![json!("a"), json!("b")]),
            Value::String("X".into()),
        );
        let arr = r.get("out").unwrap().as_array().unwrap();
        assert_eq!(arr[0].as_str(), Some("aX"));
        assert_eq!(arr[1].as_str(), Some("bX"));
    }

    #[test]
    fn prepends_when_array_on_b() {
        // Symmetric counterpart of `element_wise_on_arrays`. When the
        // scalar lands on input A and the array on input B the closure
        // must receive (lhs_scalar, rhs_elem) — so the scalar PREFIXES
        // each array element. Locks the broadcast_pairs asymmetry fix.
        let r = run(
            Value::String("X".into()),
            Value::Array(vec![json!("a"), json!("b")]),
        );
        let arr = r.get("out").unwrap().as_array().unwrap();
        assert_eq!(arr[0].as_str(), Some("Xa"));
        assert_eq!(arr[1].as_str(), Some("Xb"));
    }
}
