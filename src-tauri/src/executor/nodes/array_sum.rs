//! `array.sum` — sums all numeric elements of an array.

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
        Value::Array(xs) => {
            let mut acc = 0.0;
            for x in xs.iter() {
                let n = x.as_f64().ok_or_else(|| ExecError::TypeMismatch {
                    node: "array.sum".into(),
                    expected: "number".into(),
                    actual: value_type_name(x).into(),
                })?;
                acc += n;
            }
            f64_to_value(acc)
        }
        other => {
            return Err(ExecError::TypeMismatch {
                node: "array.sum".into(),
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
    fn sum_array() {
        let params = json!({}).as_object().unwrap().clone();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let in_val = Value::Array(vec![json!(1), json!(2), json!(3), json!(4)]);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), &in_val);
        let mut ctx = ctx_with("n1", "array.sum", &external, &mut outs, &mut logs);
        let r = exec(&inputs, &params, &mut ctx).unwrap();
        assert_eq!(r.get("out").unwrap().as_f64(), Some(10.0));
    }
}
