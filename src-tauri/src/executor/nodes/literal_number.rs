//! `literal.number` — emits a fixed number from `params.value`.

use serde_json::{Map, Value};

use crate::executor::{f64_to_value, single, EvalCtx, ExecError, NodeInputs, NodeOutputs};

pub fn exec(
    _inputs: &NodeInputs<'_>,
    params: &Map<String, Value>,
    _ctx: &mut EvalCtx<'_>,
) -> Result<NodeOutputs, ExecError> {
    let v = params
        .get("value")
        .and_then(|x| x.as_f64())
        .ok_or_else(|| ExecError::Node {
            node: "literal.number".into(),
            message: "params.value must be a number".into(),
        })?;
    Ok(single("value", f64_to_value(v)))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::{json, Value};

    use super::exec;
    use crate::executor::nodes::test_helpers::{ctx_with, empty_inputs};

    #[test]
    fn returns_value() {
        let params = json!({"value": 42}).as_object().unwrap().clone();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let null = Value::Null;
        let inputs = empty_inputs(&null);
        let mut ctx = ctx_with("n1", "literal.number", &external, &mut outs, &mut logs);
        let result = exec(&inputs, &params, &mut ctx).unwrap();
        assert_eq!(result.get("value").unwrap().as_f64(), Some(42.0));
    }
}
