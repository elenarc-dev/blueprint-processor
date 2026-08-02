//! `io.output` — writes the connected input value into the run result's
//! `outputs` map under `params.name`. Has no declared output ports.

use serde_json::{Map, Value};

use crate::executor::{empty_outputs, EvalCtx, ExecError, NodeInputs, NodeOutputs};

pub fn exec(
    inputs: &NodeInputs<'_>,
    params: &Map<String, Value>,
    ctx: &mut EvalCtx<'_>,
) -> Result<NodeOutputs, ExecError> {
    let name = params.get("name").and_then(|x| x.as_str()).unwrap_or("output");
    let null = Value::Null;
    let value = inputs.get("in").copied().unwrap_or(&null).clone();
    ctx.outputs.insert(name.to_string(), value);
    Ok(empty_outputs())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::{json, Value};

    use super::exec;
    use crate::executor::nodes::test_helpers::ctx_with;

    #[test]
    fn writes_into_ctx_outputs() {
        let params = json!({"name": "answer"}).as_object().unwrap().clone();
        let external = HashMap::new();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let in_val = Value::from(7);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), &in_val);
        let mut ctx = ctx_with("n1", "io.output", &external, &mut outs, &mut logs);
        let _ = exec(&inputs, &params, &mut ctx).unwrap();
        assert_eq!(outs.get("answer").unwrap().as_i64(), Some(7));
    }
}
