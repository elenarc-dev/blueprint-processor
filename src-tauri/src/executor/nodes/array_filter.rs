//! `array.filter` — keeps elements strictly greater than `params.threshold`.

use serde_json::{Map, Value};

use crate::executor::{single, value_type_name, EvalCtx, ExecError, NodeInputs, NodeOutputs};

pub fn exec(
    inputs: &NodeInputs<'_>,
    params: &Map<String, Value>,
    _ctx: &mut EvalCtx<'_>,
) -> Result<NodeOutputs, ExecError> {
    let threshold = params.get("threshold").and_then(|x| x.as_f64()).unwrap_or(0.0);
    let null = Value::Null;
    let arr = inputs.get("in").copied().unwrap_or(&null);
    let v = match arr {
        Value::Array(xs) => {
            let mut out: Vec<Value> = Vec::new();
            for x in xs {
                let n = x.as_f64().ok_or_else(|| ExecError::TypeMismatch {
                    node: "array.filter".into(),
                    expected: "number".into(),
                    actual: value_type_name(&x).into(),
                })?;
                if n >= threshold {
                    out.push(Value::Number(
                        serde_json::Number::from_f64(n)
                            .unwrap_or_else(|| serde_json::Number::from(0)),
                    ));
                }
            }
            Value::Array(out)
        }
        other => {
            return Err(ExecError::TypeMismatch {
                node: "array.filter".into(),
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
    fn filters_above_threshold() {
        let params = json!({"threshold": 5}).as_object().unwrap().clone();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let in_val = Value::Array(vec![json!(1), json!(5), json!(7), json!(3)]);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), &in_val);
        let mut ctx = ctx_with("n1", "array.filter", &external, &mut outs, &mut logs);
        let r = exec(&inputs, &params, &mut ctx).unwrap();
        let arr = r.get("out").unwrap().as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0].as_f64(), Some(5.0));
        assert_eq!(arr[1].as_f64(), Some(7.0));
    }
}
