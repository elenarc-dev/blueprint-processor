//! `io.logger` — appends the connected input value to the run log with
//! `params.label` and an optional severity (`params.level`, default
//! `info`). Has no declared output ports.

use serde_json::{Map, Value};

use crate::executor::{empty_outputs, EvalCtx, ExecError, LogLevel, NodeInputs, NodeOutputs};

pub fn exec(
    inputs: &NodeInputs<'_>,
    params: &Map<String, Value>,
    ctx: &mut EvalCtx<'_>,
) -> Result<NodeOutputs, ExecError> {
    let label = params.get("label").and_then(|x| x.as_str()).unwrap_or("log");
    let level = params
        .get("level")
        .and_then(|x| x.as_str())
        .map(LogLevel::from_name)
        .unwrap_or(LogLevel::Info);
    let null = Value::Null;
    let value = inputs.get("in").copied().unwrap_or(&null).clone();
    ctx.log(level, label, value);
    Ok(empty_outputs())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::{json, Value};

    use super::exec;
    use crate::executor::nodes::test_helpers::ctx_with;
    use crate::executor::LogLevel;

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
        // Absent `level` param keeps the pre-existing behaviour.
        assert_eq!(logs[0].level, LogLevel::Info);
        // Provenance is recorded by the ctx, not by the node.
        assert_eq!(logs[0].node_id, "n1");
        assert_eq!(logs[0].kind, "io.logger");
    }

    #[test]
    fn level_param_selects_severity() {
        let params = json!({"label": "bad row", "level": "error"})
            .as_object()
            .unwrap()
            .clone();
        let external = HashMap::new();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let in_val = Value::String("nope".into());
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), &in_val);
        let mut ctx = ctx_with("n2", "io.logger", &external, &mut outs, &mut logs);
        let _ = exec(&inputs, &params, &mut ctx).unwrap();
        assert_eq!(logs[0].level, LogLevel::Error);
    }

    #[test]
    fn unknown_level_falls_back_to_info() {
        assert_eq!(LogLevel::from_name("LOUDAF"), LogLevel::Info);
        assert_eq!(LogLevel::from_name("warn"), LogLevel::Warn);
        assert_eq!(LogLevel::from_name(" Debug "), LogLevel::Debug);
    }
}
