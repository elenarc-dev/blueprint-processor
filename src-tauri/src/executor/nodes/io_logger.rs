//! `io.logger` — appends the connected input value to the run result's
//! `logs` list with `params.label`. Has no declared output ports.

use serde_json::{Map, Value};

use crate::executor::{empty_outputs, EvalCtx, LogEntry, ExecError, NodeInputs, NodeOutputs};

pub fn exec(
    inputs: &NodeInputs<'_>,
    params: &Map<String, Value>,
    ctx: &mut EvalCtx<'_>,
) -> Result<NodeOutputs, ExecError> {
    let label = params.get("label").and_then(|x| x.as_str()).unwrap_or("log");
    let null = Value::Null;
    let value = inputs.get("in").copied().unwrap_or(&null).clone();
    ctx.logs.push(LogEntry {
        node_id: ctx.node_id.to_string(),
        label: label.to_string(),
        value,
    });
    Ok(empty_outputs())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::{json, Value};

    use super::exec;
    use crate::executor::nodes::test_helpers::ctx_with;

    #[test]
    fn appends_log_entry() {
        let params = json!({"label": "Hello"}).as_object().unwrap().clone();
        let external = HashMap::new();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let in_val = Value::String("world".into());
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), &in_val);
        let mut ctx = ctx_with("n1", "io.logger", &external, &mut outs, &mut logs);
        let _ = exec(&inputs, &params, &mut ctx).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].label, "Hello");
        assert_eq!(logs[0].value.as_str(), Some("world"));
    }
}
