//! `io.input` — reads from the runtime external inputs map under
//! `params.name`. Errors if a value wasn't provided for that name.

use serde_json::{Map, Value};

use crate::executor::{single, EvalCtx, ExecError, NodeInputs, NodeOutputs};

pub fn exec(
    _inputs: &NodeInputs<'_>,
    params: &Map<String, Value>,
    ctx: &mut EvalCtx<'_>,
) -> Result<NodeOutputs, ExecError> {
    let name = params.get("name").and_then(|x| x.as_str()).unwrap_or("input");
    // Strict for v1: missing external inputs is an error. Loosen later
    // when the UI generates sensible defaults.
    match ctx.external_inputs.get(name) {
        Some(v) => Ok(single("out", v.clone())),
        None => Err(ExecError::MissingInputValue { name: name.into() }),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::{json, Value};

    use super::exec;
    use super::ExecError;
    use crate::executor::nodes::test_helpers::{ctx_with, empty_inputs};

    #[test]
    fn reads_external_input() {
        let params = json!({"name": "first"}).as_object().unwrap().clone();
        let mut external = HashMap::new();
        external.insert("first".to_string(), Value::from(123));
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let null = Value::Null;
        let inputs = empty_inputs(&null);
        let mut ctx = ctx_with("n1", "io.input", &external, &mut outs, &mut logs);
        let r = exec(&inputs, &params, &mut ctx).unwrap();
        assert_eq!(r.get("out").unwrap().as_i64(), Some(123));
    }

    #[test]
    fn missing_input_errors() {
        let params = json!({"name": "missing"}).as_object().unwrap().clone();
        let external = HashMap::new();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let null = Value::Null;
        let inputs = empty_inputs(&null);
        let mut ctx = ctx_with("n1", "io.input", &external, &mut outs, &mut logs);
        let err = exec(&inputs, &params, &mut ctx).unwrap_err();
        assert!(matches!(err, ExecError::MissingInputValue { .. }));
    }
}
