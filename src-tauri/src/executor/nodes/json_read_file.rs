//! json.readFile — read a file from disk and parse its content as JSON.
//!
//! Atomic node: bundles `file.read` + `json.parse` so a user who only
//! cares about the structured value (not the raw text) can save a
//! wiring step. Errors include both I/O and parse failures with the
//! same `ExecError::Node` shape as the standalone variants.

use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::executor::{EvalCtx, ExecError, NodeInputs, NodeOutputs};

pub const KIND: &str = "json.readFile";

pub fn exec(
    _inputs: &NodeInputs,
    params: &Map<String, Value>,
    _ctx: &mut EvalCtx,
) -> Result<NodeOutputs, ExecError> {
    let path = params
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if path.is_empty() {
        return Err(ExecError::Node {
            node: KIND.into(),
            message: "params.path is empty".into(),
        });
    }

    let raw = std::fs::read_to_string(&path).map_err(|e| ExecError::Node {
        node: KIND.into(),
        message: format!("could not read '{path}': {e}"),
    })?;

    let parsed: Value =
        serde_json::from_str(&raw).map_err(|e| ExecError::Node {
            node: KIND.into(),
            message: format!("invalid JSON in '{path}': {e}"),
        })?;

    Ok(HashMap::from([("value".to_string(), parsed)]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::nodes::test_helpers::{ctx_with, empty_inputs, TmpFile};
    use serde_json::json;
    use std::fs;

    fn run_with_path(path: &str) -> Result<NodeOutputs, ExecError> {
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let ext = HashMap::new();
        let inputs = empty_inputs(&Value::Null);
        let mut params = Map::new();
        params.insert("path".to_string(), Value::String(path.into()));
        let mut ctx = ctx_with("n1", KIND, &ext, &mut outs, &mut logs);
        exec(&inputs, &params, &mut ctx)
    }

    #[test]
    fn reads_and_parses_object_file() {
        let tmp = TmpFile::new("json-read-obj");
        fs::write(&tmp.0, r#"{"k": "v", "n": 42}"#).unwrap();
        let out = run_with_path(tmp.0.to_str().unwrap()).expect("ok");
        assert_eq!(out.get("value"), Some(&json!({"k": "v", "n": 42})));
    }

    #[test]
    fn reads_and_parses_array_file() {
        let tmp = TmpFile::new("json-read-arr");
        fs::write(&tmp.0, "[1, 2, 3]\n").unwrap();
        let out = run_with_path(tmp.0.to_str().unwrap()).expect("ok");
        assert_eq!(out.get("value"), Some(&json!([1, 2, 3])));
    }

    #[test]
    fn missing_file_errors() {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "bp-test-definitely-missing-{}.json",
            std::process::id()
        ));
        let err = run_with_path(p.to_str().unwrap()).unwrap_err();
        match err {
            ExecError::Node { node, message } => {
                assert_eq!(node, KIND);
                assert!(
                    message.contains("could not read"),
                    "msg was: {message}"
                );
            }
            other => panic!("expected Node, got {other:?}"),
        }
    }

    #[test]
    fn invalid_json_in_file_errors() {
        let tmp = TmpFile::new("json-read-bad");
        fs::write(&tmp.0, "this is not json").unwrap();
        let err = run_with_path(tmp.0.to_str().unwrap()).unwrap_err();
        match err {
            ExecError::Node { node, message } => {
                assert_eq!(node, KIND);
                assert!(
                    message.contains("invalid JSON"),
                    "msg was: {message}"
                );
            }
            other => panic!("expected Node, got {other:?}"),
        }
    }

    #[test]
    fn empty_path_errors() {
        let err = run_with_path("").unwrap_err();
        match err {
            ExecError::Node { node, message } => {
                assert_eq!(node, KIND);
                assert!(message.contains("empty"), "msg was: {message}");
            }
            other => panic!("expected Node, got {other:?}"),
        }
    }
}
