//! `string.length` — character count of a string (or array of strings).

use serde_json::{Map, Value};

use crate::executor::{f64_to_value, single, value_to_string, EvalCtx, ExecError, NodeInputs, NodeOutputs};

pub fn exec(
    inputs: &NodeInputs<'_>,
    _params: &Map<String, Value>,
    _ctx: &mut EvalCtx<'_>,
) -> Result<NodeOutputs, ExecError> {
    let null = Value::Null;
    let s = inputs.get("in").copied().unwrap_or(&null);
    let v = match s {
        Value::Array(xs) => Value::Array(
            xs.iter()
                .map(|v| {
                    let s = value_to_string(v);
                    f64_to_value(s.chars().count() as f64)
                })
                .collect(),
        ),
        _ => f64_to_value(value_to_string(s).chars().count() as f64),
    };
    Ok(single("out", v))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::{json, Value};

    use super::exec;
    use crate::executor::nodes::test_helpers::ctx_with;

    #[test]
    fn char_count_scalar() {
        let params = json!({}).as_object().unwrap().clone();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let in_val = Value::String("hello".into());
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), &in_val);
        let mut ctx = ctx_with("n1", "string.length", &external, &mut outs, &mut logs);
        let r = exec(&inputs, &params, &mut ctx).unwrap();
        assert_eq!(r.get("out").unwrap().as_f64(), Some(5.0));
    }
}
