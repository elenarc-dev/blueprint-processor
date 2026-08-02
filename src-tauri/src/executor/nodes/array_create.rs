//! `array.create` — bundles N runtime inputs (`field_1`..`field_N`)
//! decided by `params.count` into a fresh `Value::Array`. Missing inputs
//! are coerced to `Value::Null` so the array length always matches
//! `count` even when some handles are unconnected.

use serde_json::{Map, Value};

use crate::executor::{single, EvalCtx, ExecError, NodeInputs, NodeOutputs};

pub fn exec(
    inputs: &NodeInputs<'_>,
    params: &Map<String, Value>,
    _ctx: &mut EvalCtx<'_>,
) -> Result<NodeOutputs, ExecError> {
    let raw_count = params.get("count").and_then(|x| x.as_u64()).unwrap_or(1);
    let count = raw_count.max(1) as usize;
    let null = Value::Null;
    let mut out: Vec<Value> = Vec::with_capacity(count);
    for i in 1..=count {
        let key = format!("field_{i}");
        let v = inputs.get(&key).copied().unwrap_or(&null).clone();
        out.push(v);
    }
    Ok(single("out", Value::Array(out)))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::{json, Value};

    use super::exec;
    use crate::executor::nodes::test_helpers::ctx_with;

    #[test]
    fn bundles_default_one() {
        let params = json!({"count": 1}).as_object().unwrap().clone();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let v = Value::from(42);
        let mut inputs = HashMap::new();
        inputs.insert("field_1".to_string(), &v);
        let mut ctx = ctx_with("n1", "array.create", &external, &mut outs, &mut logs);
        let r = exec(&inputs, &params, &mut ctx).unwrap();
        let arr = r.get("out").unwrap().as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0].as_i64(), Some(42));
    }

    #[test]
    fn bundles_three_fields() {
        let params = json!({"count": 3}).as_object().unwrap().clone();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let v1 = Value::from(1);
        let v2 = Value::String("two".into());
        let v3 = Value::Bool(true);
        let mut inputs = HashMap::new();
        inputs.insert("field_1".to_string(), &v1);
        inputs.insert("field_2".to_string(), &v2);
        inputs.insert("field_3".to_string(), &v3);
        let mut ctx = ctx_with("n1", "array.create", &external, &mut outs, &mut logs);
        let r = exec(&inputs, &params, &mut ctx).unwrap();
        let arr = r.get("out").unwrap().as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0].as_i64(), Some(1));
        assert_eq!(arr[1].as_str(), Some("two"));
        assert_eq!(arr[2].as_bool(), Some(true));
    }

    #[test]
    fn missing_inputs_become_null() {
        let params = json!({"count": 3}).as_object().unwrap().clone();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let v1 = Value::from(1);
        let mut inputs = HashMap::new();
        inputs.insert("field_1".to_string(), &v1);
        // field_2 and field_3 deliberately absent
        let mut ctx = ctx_with("n1", "array.create", &external, &mut outs, &mut logs);
        let r = exec(&inputs, &params, &mut ctx).unwrap();
        let arr = r.get("out").unwrap().as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0].as_i64(), Some(1));
        assert!(arr[1].is_null());
        assert!(arr[2].is_null());
    }

    #[test]
    fn zero_count_falls_back_to_one() {
        // params.count = 0 (e.g. user typed it manually). We never want
        // a zero-length output here, so clamp to 1 in the Rust layer.
        let params = json!({"count": 0}).as_object().unwrap().clone();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let v = Value::from(99);
        let mut inputs = HashMap::new();
        inputs.insert("field_1".to_string(), &v);
        let mut ctx = ctx_with("n1", "array.create", &external, &mut outs, &mut logs);
        let r = exec(&inputs, &params, &mut ctx).unwrap();
        assert_eq!(r.get("out").unwrap().as_array().unwrap().len(), 1);
    }
}
