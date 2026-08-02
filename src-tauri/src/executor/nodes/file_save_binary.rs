//! `file.saveBinary` — writes one or many base64-encoded payloads to
//! disk at the matching one-or-many paths supplied via `names`.
//!
//! Shape pairing (delegated to `executor::align_binary_with_paths` so
//! the same rules apply in any future binary-in/binary-out node):
//!
//! | files        | names        | behaviour           |
//! |--------------|--------------|---------------------|
//! | Scalar       | Scalar       | one save            |
//! | Scalar       | Array(N)     | broadcast: same payload to N paths |
//! | Array(N)     | Array(N)     | N parallel saves (lengths must match) |
//! | Array(N)     | Scalar       | **error** (avoids overwriting the same path N times) |
//!
//! **Parent-directory handling:** mirrors `file.write` — if `names[i]`
//! has a non-empty parent, the directory is created on demand so the
//! user does not have to pre-stage the output folder.

use serde_json::{Map, Value};

use crate::executor::{
    align_binary_with_paths, decode_b64, single, EvalCtx, ExecError, NodeInputs, NodeOutputs,
};

pub fn exec(
    inputs: &NodeInputs<'_>,
    _params: &Map<String, Value>,
    _ctx: &mut EvalCtx<'_>,
) -> Result<NodeOutputs, ExecError> {
    let null = Value::Null;
    let files = inputs.get("files").copied().unwrap_or(&null);
    let names = inputs.get("names").copied().unwrap_or(&null);

    let pairs = align_binary_with_paths(files, names, "file.saveBinary")?;
    let mut saved: Vec<Value> = Vec::with_capacity(pairs.len());

    for (file_val, name_val) in pairs {
        // `Value::String.as_str()` works directly on the first arm and
        // on any other variant falls through to `Option::None`. There
        // is no need for an explicit `match` here — a single
        // `as_str().ok_or_else(...)` reads the same intent in fewer
        // lines and avoids the earlier dead-duplicate-arm pattern.
        let path = name_val.as_str().ok_or_else(|| ExecError::Node {
            node: "file.saveBinary".into(),
            message: "every `names` entry must be a string path".into(),
        })?.to_string();
        let b64 = file_val.as_str().ok_or_else(|| ExecError::Node {
            node: "file.saveBinary".into(),
            message: "every `files` entry must be a base64-encoded string".into(),
        })?;
        let bytes = decode_b64(b64).map_err(|e| ExecError::Node {
            node: "file.saveBinary".into(),
            message: format!("invalid base64 payload: {e}"),
        })?;

        // Mirror `file.write`: ensure the parent directory exists.
        let p = std::path::Path::new(&path);
        if let Some(parent) = p.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| ExecError::Node {
                    node: "file.saveBinary".into(),
                    message: format!("could not create parent for '{path}': {e}"),
                })?;
            }
        }
        std::fs::write(&path, &bytes).map_err(|e| ExecError::Node {
            node: "file.saveBinary".into(),
            message: format!("could not write '{path}': {e}"),
        })?;
        saved.push(Value::String(path));
    }

    // Surface the saved paths so downstream logging / file.iterate /
    // json.* nodes can consume them. Shape mirrors the names input so
    // a scalar stay scalar, an array stays array.
    let value = if saved.len() == 1 {
        saved.into_iter().next().unwrap()
    } else {
        Value::Array(saved)
    };
    Ok(single("saved", value))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;

    use serde_json::{json, Value};

    use super::exec;
    use crate::executor::ExecError;
    use crate::executor::LogEntry;
    use crate::executor::nodes::test_helpers::{ctx_with, TmpFile};

    /// Test fixture: build a `ctx_with` bundle. Note that
    /// `outs` here is `EvalCtx.outputs` — the terminal-outputs buffer
    /// reserved for `io.output`. **It does NOT receive this node's
    /// output port values**; those ride on the return value of `exec`.
    /// Earlier tests mistakenly asserted `outs.get("saved")` and
    /// panicked because the buffer was empty. The pattern below
    /// captures `r = exec(...).unwrap()` and checks `r.get(...)`
    /// instead — see `array_push`'s tests for the same shape.
    fn ctx() -> (HashMap<String, Value>, Vec<LogEntry>, HashMap<String, Value>) {
        (HashMap::new(), Vec::new(), HashMap::new())
    }

    fn b64(s: &[u8]) -> String {
        use base64::engine::general_purpose::STANDARD as B64;
        use base64::Engine as _;
        B64.encode(s)
    }

    #[test]
    fn saves_single_pair() {
        let (mut outs, mut logs, external) = ctx();
        let file = TmpFile::new("sb_single");
        let path = file.0.to_string_lossy().to_string();
        let file_val = Value::String(b64(b"hello binary"));
        let name_val = Value::String(path.clone());
        let mut inputs = HashMap::new();
        inputs.insert("files".to_string(), &file_val);
        inputs.insert("names".to_string(), &name_val);
        let params = json!({}).as_object().unwrap().clone();
        let mut ctx_inst = ctx_with("n1", "file.saveBinary", &external, &mut outs, &mut logs);
        let r = exec(&inputs, &params, &mut ctx_inst).unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"hello binary");
        // The `saved` port rides on the return value, not on ctx.outputs.
        assert_eq!(r.get("saved").unwrap().as_str(), Some(path.as_str()));
    }

    #[test]
    fn saves_array_pair_with_equal_length() {
        let (mut outs, mut logs, external) = ctx();
        let f1 = TmpFile::new("sb_arr1");
        let f2 = TmpFile::new("sb_arr2");
        let p1 = f1.0.to_string_lossy().to_string();
        let p2 = f2.0.to_string_lossy().to_string();
        let file_val = Value::Array(vec![
            Value::String(b64(b"first body")),
            Value::String(b64(b"second body")),
        ]);
        let name_val = Value::Array(vec![
            Value::String(p1.clone()),
            Value::String(p2.clone()),
        ]);
        let mut inputs = HashMap::new();
        inputs.insert("files".to_string(), &file_val);
        inputs.insert("names".to_string(), &name_val);
        let params = json!({}).as_object().unwrap().clone();
        let mut ctx_inst = ctx_with("n1", "file.saveBinary", &external, &mut outs, &mut logs);
        let r = exec(&inputs, &params, &mut ctx_inst).unwrap();

        assert_eq!(fs::read(&p1).unwrap(), b"first body");
        assert_eq!(fs::read(&p2).unwrap(), b"second body");
        let arr = r.get("saved").unwrap().as_array().unwrap();
        assert_eq!(arr.len(), 2);
    }

    #[test]
    fn broadcasts_scalar_file_to_array_of_names() {
        let (mut outs, mut logs, external) = ctx();
        let f1 = TmpFile::new("sb_bc1");
        let f2 = TmpFile::new("sb_bc2");
        let p1 = f1.0.to_string_lossy().to_string();
        let p2 = f2.0.to_string_lossy().to_string();
        let file_val = Value::String(b64(b"broadcast me"));
        let name_val = Value::Array(vec![
            Value::String(p1.clone()),
            Value::String(p2.clone()),
        ]);
        let mut inputs = HashMap::new();
        inputs.insert("files".to_string(), &file_val);
        inputs.insert("names".to_string(), &name_val);
        let params = json!({}).as_object().unwrap().clone();
        let mut ctx_inst = ctx_with("n1", "file.saveBinary", &external, &mut outs, &mut logs);
        exec(&inputs, &params, &mut ctx_inst).unwrap();

        // Same bytes end up in both destinations.
        assert_eq!(fs::read(&p1).unwrap(), b"broadcast me");
        assert_eq!(fs::read(&p2).unwrap(), b"broadcast me");
    }

    #[test]
    fn errors_on_mismatched_array_lengths() {
        let (mut outs, mut logs, external) = ctx();
        let f1 = TmpFile::new("sb_mm1");
        let f2 = TmpFile::new("sb_mm2");
        let p1 = f1.0.to_string_lossy().to_string();
        let p2 = f2.0.to_string_lossy().to_string();
        let file_val = Value::Array(vec![Value::String(b64(b"only-one"))]);
        let name_val = Value::Array(vec![
            Value::String(p1.clone()),
            Value::String(p2.clone()),
        ]);
        let mut inputs = HashMap::new();
        inputs.insert("files".to_string(), &file_val);
        inputs.insert("names".to_string(), &name_val);
        let params = json!({}).as_object().unwrap().clone();
        let mut ctx_inst = ctx_with("n1", "file.saveBinary", &external, &mut outs, &mut logs);
        let err = exec(&inputs, &params, &mut ctx_inst).unwrap_err();
        assert!(matches!(err, ExecError::Node { .. }));
    }

    #[test]
    fn errors_on_array_files_to_scalar_name() {
        let (mut outs, mut logs, external) = ctx();
        let f = TmpFile::new("sb_arr_to_one");
        let p = f.0.to_string_lossy().to_string();
        let file_val = Value::Array(vec![
            Value::String(b64(b"a")),
            Value::String(b64(b"b")),
        ]);
        let name_val = Value::String(p);
        let mut inputs = HashMap::new();
        inputs.insert("files".to_string(), &file_val);
        inputs.insert("names".to_string(), &name_val);
        let params = json!({}).as_object().unwrap().clone();
        let mut ctx_inst = ctx_with("n1", "file.saveBinary", &external, &mut outs, &mut logs);
        let err = exec(&inputs, &params, &mut ctx_inst).unwrap_err();
        assert!(matches!(err, ExecError::Node { .. }));
    }

    #[test]
    fn errors_on_disconnected_inputs() {
        let (mut outs, mut logs, external) = ctx();
        let params = json!({}).as_object().unwrap().clone();
        let file_val = Value::Null;
        let name_val = Value::Null;
        let mut inputs = HashMap::new();
        inputs.insert("files".to_string(), &file_val);
        inputs.insert("names".to_string(), &name_val);
        let mut ctx_inst = ctx_with("n1", "file.saveBinary", &external, &mut outs, &mut logs);
        let err = exec(&inputs, &params, &mut ctx_inst).unwrap_err();
        assert!(matches!(err, ExecError::Node { .. }));
    }

    #[test]
    fn errors_on_invalid_base64_payload() {
        let (mut outs, mut logs, external) = ctx();
        let f = TmpFile::new("sb_bad_b64");
        let p = f.0.to_string_lossy().to_string();
        let file_val = Value::String("not-base64@@@".into());
        let name_val = Value::String(p);
        let mut inputs = HashMap::new();
        inputs.insert("files".to_string(), &file_val);
        inputs.insert("names".to_string(), &name_val);
        let params = json!({}).as_object().unwrap().clone();
        let mut ctx_inst = ctx_with("n1", "file.saveBinary", &external, &mut outs, &mut logs);
        let err = exec(&inputs, &params, &mut ctx_inst).unwrap_err();
        assert!(matches!(err, ExecError::Node { .. }));
    }

    #[test]
    fn creates_parent_directories_for_nested_paths() {
        // Mirrors `file.write::creates_parent_directories` — confirms
        // `create_dir_all` actually fires and deep nested paths work.
        let (mut outs, mut logs, external) = ctx();
        let root = std::env::temp_dir().join(format!(
            "bp-sb-nested-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let nested = root.join("a/b/c/deep.bin");
        let path = nested.to_string_lossy().to_string();

        let file_val = Value::String(b64(b"deep bytes"));
        let name_val = Value::String(path.clone());
        let mut inputs = HashMap::new();
        inputs.insert("files".to_string(), &file_val);
        inputs.insert("names".to_string(), &name_val);
        let params = json!({}).as_object().unwrap().clone();
        let mut ctx_inst = ctx_with("n1", "file.saveBinary", &external, &mut outs, &mut logs);
        let _r = exec(&inputs, &params, &mut ctx_inst).unwrap();

        assert!(nested.exists());
        assert_eq!(fs::read(&nested).unwrap(), b"deep bytes");
        // Cleanup of the nested tree (TmpFile's Drop only unlinks files).
        fs::remove_file(&nested).ok();
        let _ = fs::remove_dir_all(&root);
    }
}
