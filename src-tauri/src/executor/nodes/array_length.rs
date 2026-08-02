//! `array.length` — number of elements in an array (errors on non-array).

use serde_json::{Map, Value};

use crate::executor::{f64_to_value, single, value_type_name, EvalCtx, ExecError, NodeInputs, NodeOutputs};

pub fn exec(
    inputs: &NodeInputs<'_>,
    _params: &Map<String, Value>,
    _ctx: &mut EvalCtx<'_>,
) -> Result<NodeOutputs, ExecError> {
    let null = Value::Null;
    let arr = inputs.get("in").copied().unwrap_or(&null);
    let v = match arr {
        Value::Array(xs) => f64_to_value(xs.len() as f64),
        other => {
            return Err(ExecError::TypeMismatch {
                node: "array.length".into(),
                expected: "array".into(),
                actual: value_type_name(other).into(),
            })
        }
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
    fn array_length_scalar() {
        let params = json!({}).as_object().unwrap().clone();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let in_val = Value::Array(vec![json!(1), json!(2), json!(3)]);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), &in_val);
        let mut ctx = ctx_with("n1", "array.length", &external, &mut outs, &mut logs);
        let r = exec(&inputs, &params, &mut ctx).unwrap();
        assert_eq!(r.get("out").unwrap().as_f64(), Some(3.0));
    }
}
