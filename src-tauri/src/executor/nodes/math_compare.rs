//! `math.compare` — compares two numeric inputs with `params.op`
//! (eq | neq | gt | gte | lt | lte). Broadcasts over equal-length arrays.

use serde_json::{Map, Value};

use crate::executor::{single, value_type_name, EvalCtx, ExecError, NodeInputs, NodeOutputs};

pub fn exec(
    inputs: &NodeInputs<'_>,
    params: &Map<String, Value>,
    _ctx: &mut EvalCtx<'_>,
) -> Result<NodeOutputs, ExecError> {
    let op_str = params.get("op").and_then(|x| x.as_str()).unwrap_or("eq");
    let null = Value::Null;
    let a = inputs.get("a").copied().unwrap_or(&null);
    let b = inputs.get("b").copied().unwrap_or(&null);

    // We deliberately do NOT use `broadcast_numeric` here because its
    // closure signature forces `Result<f64, _>` — boolean outputs would
    // be silently re-coerced to `Number`. We inline the broadcasting
    // pattern so the output port stays a real `Bool` (or `Array<Bool>`).
    let pair = |av: f64, bv: f64| -> Result<bool, ExecError> {
        Ok(match op_str {
            "eq" => av == bv,
            "neq" => av != bv,
            "gt" => av > bv,
            "gte" => av >= bv,
            "lt" => av < bv,
            "lte" => av <= bv,
            other => {
                return Err(ExecError::Node {
                    node: "math.compare".into(),
                    message: format!("unknown op '{other}'"),
                })
            }
        })
    };

    let v = match (a, b) {
        (Value::Array(xs), Value::Array(ys)) if xs.len() == ys.len() => {
            let mut out: Vec<Value> = Vec::with_capacity(xs.len());
            for (xv, yv) in xs.iter().zip(ys) {
                let xn = xv.as_f64().ok_or_else(|| ExecError::TypeMismatch {
                    node: "math.compare".into(),
                    expected: "number".into(),
                    actual: value_type_name(xv).into(),
                })?;
                let yn = yv.as_f64().ok_or_else(|| ExecError::TypeMismatch {
                    node: "math.compare".into(),
                    expected: "number".into(),
                    actual: value_type_name(yv).into(),
                })?;
                out.push(Value::Bool(pair(xn, yn)?));
            }
            Value::Array(out)
        }
        _ => Value::Bool(pair(
            a.as_f64().ok_or_else(|| ExecError::TypeMismatch {
                node: "math.compare".into(),
                expected: "number".into(),
                actual: value_type_name(a).into(),
            })?,
            b.as_f64().ok_or_else(|| ExecError::TypeMismatch {
                node: "math.compare".into(),
                expected: "number".into(),
                actual: value_type_name(b).into(),
            })?,
        )?),
    };
    Ok(single("result", v))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::{json, Value};

    use super::exec;
    use crate::executor::nodes::test_helpers::ctx_with;

    fn run_op(op: &str, a: Value, b: Value) -> HashMap<String, Value> {
        let params = json!({"op": op}).as_object().unwrap().clone();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let a_val = a;
        let b_val = b;
        let mut inputs = HashMap::new();
        inputs.insert("a".to_string(), &a_val);
        inputs.insert("b".to_string(), &b_val);
        let mut ctx = ctx_with("n1", "math.compare", &external, &mut outs, &mut logs);
        exec(&inputs, &params, &mut ctx).unwrap()
    }

    #[test]
    fn lte_scalar_equal() {
        let r = run_op("lte", Value::from(3), Value::from(3));
        assert_eq!(r.get("result").unwrap().as_bool(), Some(true));
    }

    #[test]
    fn gt_over_arrays() {
        let r = run_op(
            "gt",
            Value::Array(vec![json!(5), json!(3), json!(5)]),
            Value::Array(vec![json!(3), json!(5), json!(5)]),
        );
        let arr = r.get("result").unwrap().as_array().unwrap();
        assert_eq!(
            arr.iter().map(|v| v.as_bool().unwrap()).collect::<Vec<_>>(),
            vec![true, false, false]
        );
    }
}
