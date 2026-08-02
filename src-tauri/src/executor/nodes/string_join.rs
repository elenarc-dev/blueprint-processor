//! `string.join` — joins an array of values into a single string
//! separated by `params.sep` (default ","). Numbers/bools coerce via
//! `value_to_string`.

use serde_json::{Map, Value};

use crate::executor::{single, value_to_string, value_type_name, EvalCtx, ExecError, NodeInputs, NodeOutputs};

pub fn exec(
    inputs: &NodeInputs<'_>,
    params: &Map<String, Value>,
    _ctx: &mut EvalCtx<'_>,
) -> Result<NodeOutputs, ExecError> {
    let sep = params.get("sep").and_then(|x| x.as_str()).unwrap_or(",");
    let null = Value::Null;
    let arr = inputs.get("arr").copied().unwrap_or(&null);
    let value = match arr {
        Value::Array(xs) => {
            let parts: Vec<String> = xs.iter().map(value_to_string).collect();
            Value::String(parts.join(sep))
        }
        Value::Null => Value::String(String::new()),
        other => {
            return Err(ExecError::TypeMismatch {
                node: "string.join".into(),
                expected: "array".into(),
                actual: value_type_name(other).into(),
            })
        }
    };
    Ok(single("out", value))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::{json, Value};

    use super::exec;
    use crate::executor::nodes::test_helpers::ctx_with;

    fn run(sep: &str, arr: Value) -> HashMap<String, Value> {
        let params = json!({"sep": sep}).as_object().unwrap().clone();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let in_val = arr;
        let mut inputs = HashMap::new();
        inputs.insert("arr".to_string(), &in_val);
        let mut ctx = ctx_with("n1", "string.join", &external, &mut outs, &mut logs);
        exec(&inputs, &params, &mut ctx).unwrap()
    }

    #[test]
    fn string_array() {
        let r = run(", ", Value::Array(vec![json!("a"), json!("b"), json!("c")]));
        assert_eq!(r.get("out").unwrap().as_str(), Some("a, b, c"));
    }

    #[test]
    fn numeric_array_coerces() {
        let r = run("-", Value::Array(vec![json!(1), json!(2), json!(3)]));
        assert_eq!(r.get("out").unwrap().as_str(), Some("1-2-3"));
    }
}
