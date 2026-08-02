//! `string.split` — splits a string by `params.sep` (default ",") into
//! an array of strings.

use serde_json::{Map, Value};

use crate::executor::{single, value_to_string, EvalCtx, ExecError, NodeInputs, NodeOutputs};

pub fn exec(
    inputs: &NodeInputs<'_>,
    params: &Map<String, Value>,
    _ctx: &mut EvalCtx<'_>,
) -> Result<NodeOutputs, ExecError> {
    let sep = params.get("sep").and_then(|x| x.as_str()).unwrap_or(",");
    let null = Value::Null;
    let raw = inputs.get("in").copied().unwrap_or(&null);
    let pieces: Vec<String> = match raw {
        Value::String(s) => s.split(sep).map(|p| p.to_string()).collect(),
        // For arrays/objects, fall back to JSON stringification + split.
        other => value_to_string(other).split(sep).map(|p| p.to_string()).collect(),
    };
    let arr: Vec<Value> = pieces.into_iter().map(Value::String).collect();
    Ok(single("out", Value::Array(arr)))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::{json, Value};

    use super::exec;
    use crate::executor::nodes::test_helpers::ctx_with;

    #[test]
    fn split_csv() {
        let params = json!({"sep": ","}).as_object().unwrap().clone();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let in_val = Value::String("a,b,c".into());
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), &in_val);
        let mut ctx = ctx_with("n1", "string.split", &external, &mut outs, &mut logs);
        let r = exec(&inputs, &params, &mut ctx).unwrap();
        let arr = r.get("out").unwrap().as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0].as_str(), Some("a"));
        assert_eq!(arr[1].as_str(), Some("b"));
        assert_eq!(arr[2].as_str(), Some("c"));
    }
}
