//! `literal.boolean` — emits a fixed boolean from `params.value`.

use serde_json::{Map, Value};

use crate::executor::{single, EvalCtx, ExecError, NodeInputs, NodeOutputs};

pub fn exec(
    _inputs: &NodeInputs<'_>,
    params: &Map<String, Value>,
    _ctx: &mut EvalCtx<'_>,
) -> Result<NodeOutputs, ExecError> {
    let v = params
        .get("value")
        .and_then(|x| x.as_bool())
        .unwrap_or(false);
    Ok(single("value", Value::Bool(v)))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::{json, Value};

    use super::exec;
    use crate::executor::nodes::test_helpers::{ctx_with, empty_inputs};

    #[test]
    fn returns_value() {
        let params = json!({"value": true}).as_object().unwrap().clone();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let null = Value::Null;
        let inputs = empty_inputs(&null);
        let mut ctx = ctx_with("n1", "literal.boolean", &external, &mut outs, &mut logs);
        let result = exec(&inputs, &params, &mut ctx).unwrap();
        assert_eq!(result.get("value").unwrap().as_bool(), Some(true));
    }
}
