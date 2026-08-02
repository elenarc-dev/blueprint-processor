//! `file.write` — writes a string to disk at `params.path`.
//! Parent directories are created on demand so the user can drop a
//! deeply nested path without pre-creating folders.
//! Returns empty outputs (side-effect only — pair it with a `string.*`
//! node upstream if you want to surface the written payload).

use serde_json::{Map, Value};

use crate::executor::{
    empty_outputs, value_to_string, EvalCtx, ExecError, NodeInputs, NodeOutputs,
};

pub fn exec(
    inputs: &NodeInputs<'_>,
    params: &Map<String, Value>,
    _ctx: &mut EvalCtx<'_>,
) -> Result<NodeOutputs, ExecError> {
    let path = params
        .get("path")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    if path.is_empty() {
        return Err(ExecError::Node {
            node: "file.write".into(),
            message: "params.path is empty".into(),
        });
    }

    // Reuse the canonical weak-coerce helper so behaviour matches
    // `string.concat` and `io_logger` exactly: null → empty string,
    // bool / number → to_string, string → clone, otherwise → JSON repr.
    let null = Value::Null;
    let text = value_to_string(inputs.get("content").copied().unwrap_or(&null));

    let p = std::path::Path::new(&path);
    if let Some(parent) = p.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| ExecError::Node {
                node: "file.write".into(),
                message: format!("could not create parent dir for '{path}': {e}"),
            })?;
        }
    }
    std::fs::write(&path, text.as_bytes()).map_err(|e| ExecError::Node {
        node: "file.write".into(),
        message: format!("could not write '{path}': {e}"),
    })?;

    Ok(empty_outputs())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;

    use serde_json::{json, Value};

    use super::exec;
    use crate::executor::nodes::test_helpers::{ctx_with, TmpFile};

    fn params_for(path: &str) -> serde_json::Map<String, Value> {
        json!({ "path": path }).as_object().unwrap().clone()
    }

    #[test]
    fn writes_file() {
        let file = TmpFile::new("write_basic");
        let path_str = file.0.to_string_lossy().to_string();
        let body = "hello world\nsecond line";

        let params = params_for(&path_str);
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let content_val = Value::String(body.into());
        let mut inputs = HashMap::new();
        inputs.insert("content".to_string(), &content_val);
        let mut ctx = ctx_with("n1", "file.write", &external, &mut outs, &mut logs);
        exec(&inputs, &params, &mut ctx).unwrap();

        assert_eq!(fs::read_to_string(&file.0).unwrap(), body);
        // file guard drops here and `fs::remove_file` runs.
    }

    #[test]
    fn creates_parent_directories() {
        let guard_root = TmpFile::new("write_nested_root");
        let nested = guard_root.0.join("a/b/c/deep.txt");
        let path_str = nested.to_string_lossy().to_string();

        let params = params_for(&path_str);
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let content_val = Value::String("deep content".into());
        let mut inputs = HashMap::new();
        inputs.insert("content".to_string(), &content_val);
        let mut ctx = ctx_with("n1", "file.write", &external, &mut outs, &mut logs);
        exec(&inputs, &params, &mut ctx).unwrap();

        assert!(nested.exists());
        assert_eq!(fs::read_to_string(&nested).unwrap(), "deep content");
        // Guard's Drop only unlinks the file, not the directory tree.
        // Manual cleanup of the nested tree.
        fs::remove_file(&nested).ok();
        let _ = fs::remove_dir_all(&guard_root.0);
    }

    #[test]
    fn coerces_non_string_content_to_string() {
        let file = TmpFile::new("write_coerce");
        let path_str = file.0.to_string_lossy().to_string();

        let params = params_for(&path_str);
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        // Number → "42" via value_to_string (matches `string.concat`'s rule).
        let content_val = Value::from(42);
        let mut inputs = HashMap::new();
        inputs.insert("content".to_string(), &content_val);
        let mut ctx = ctx_with("n1", "file.write", &external, &mut outs, &mut logs);
        exec(&inputs, &params, &mut ctx).unwrap();

        assert_eq!(fs::read_to_string(&file.0).unwrap(), "42");
    }

    #[test]
    fn coerces_bool_content() {
        let file = TmpFile::new("write_bool");
        let path_str = file.0.to_string_lossy().to_string();

        let params = params_for(&path_str);
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let content_val = Value::Bool(true);
        let mut inputs = HashMap::new();
        inputs.insert("content".to_string(), &content_val);
        let mut ctx = ctx_with("n1", "file.write", &external, &mut outs, &mut logs);
        exec(&inputs, &params, &mut ctx).unwrap();

        assert_eq!(fs::read_to_string(&file.0).unwrap(), "true");
    }

    #[test]
    fn errors_on_empty_path() {
        let params = json!({ "path": "" }).as_object().unwrap().clone();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let content_val = Value::String("x".into());
        let mut inputs = HashMap::new();
        inputs.insert("content".to_string(), &content_val);
        let mut ctx = ctx_with("n1", "file.write", &external, &mut outs, &mut logs);
        assert!(matches!(
            exec(&inputs, &params, &mut ctx).unwrap_err(),
            crate::executor::ExecError::Node { .. }
        ));
    }
}
