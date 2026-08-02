//! `math.subtract` — subtracts b from a (broadcasts over arrays).

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
    let v = broadcast_numeric(a, b, |x, y| Ok::<f64, ExecError>(x - y), "math.subtract")?;
    Ok(single("diff", v))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::{json, Value};

    use super::exec;
    use crate::executor::nodes::test_helpers::ctx_with;

    #[test]
    fn scalar_subtraction() {
        let params = json!({}).as_object().unwrap().clone();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let a_val = Value::from(10);
        let b_val = Value::from(4);
        let mut inputs = HashMap::new();
        inputs.insert("a".to_string(), &a_val);
        inputs.insert("b".to_string(), &b_val);
        let mut ctx = ctx_with("n1", "math.subtract", &external, &mut outs, &mut logs);
        let result = exec(&inputs, &params, &mut ctx).unwrap();
        assert_eq!(result.get("diff").unwrap().as_f64(), Some(6.0));
    }
}
