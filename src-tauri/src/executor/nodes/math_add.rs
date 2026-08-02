//! `math.add` — sums two numeric inputs (broadcasts over arrays).

use serde_json::{Map, Value};

use crate::executor::broadcast::broadcast_numeric;
use crate::executor::{single, EvalCtx, ExecError, NodeInputs, NodeOutputs};

pub fn exec(
    inputs: &NodeInputs<'_>,
    _params: &Map<String, Value>,
    _ctx: &mut EvalCtx<'_>,
) -> Result<NodeOutputs, ExecError> {
    let null = Value::Null;
    let a = inputs.get("a").copied().unwrap_or(&null);
    let b = inputs.get("b").copied().unwrap_or(&null);
    let v = broadcast_numeric(a, b, |x, y| Ok::<f64, ExecError>(x + y), "math.add")?;
    Ok(single("sum", v))
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
        let mut ctx = ctx_with("n1", "math.add", &external, &mut outs, &mut logs);
        exec(&inputs, &params, &mut ctx).unwrap()
    }

    #[test]
    fn scalar_addition() {
        let result = run(Value::from(5), Value::from(3));
        assert_eq!(result.get("sum").unwrap().as_f64(), Some(8.0));
    }

    #[test]
    fn broadcasts_arrays() {
        let result = run(
            Value::Array(vec![json!(1), json!(2), json!(3)]),
            Value::Array(vec![json!(10), json!(20), json!(30)]),
        );
        let arr = result.get("sum").unwrap().as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0].as_f64(), Some(11.0));
        assert_eq!(arr[1].as_f64(), Some(22.0));
        assert_eq!(arr[2].as_f64(), Some(33.0));
    }

    #[test]
    fn scalar_plus_array() {
        let result = run(
            Value::Array(vec![json!(1), json!(2), json!(3)]),
            Value::from(10),
        );
        let arr = result.get("sum").unwrap().as_array().unwrap();
        assert_eq!(
            arr.iter().map(|v| v.as_f64().unwrap()).collect::<Vec<_>>(),
            vec![11.0, 12.0, 13.0]
        );
    }
}
