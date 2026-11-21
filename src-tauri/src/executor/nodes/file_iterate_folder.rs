//! `file.iterate` — lists the entries of a folder.
//!
//! Two output ports per scan, kept parallel (`paths[i]` is the full path of
//! `filenames[i]`):
//! - `filenames` — array<string> with each entry's name only.
//! - `paths`     — array<string> with each entry's full path, ready to feed
//!                 `file.read`, `file.delete` or another `file.iterate`.
//!
//! The folder comes from the `path` input port when wired, otherwise from
//! `params.path`. Every entry is returned — files and subdirectories alike —
//! because this is a `read_dir` wrapper, not a recursive walker. The order is
//! whatever the OS returns (not sorted); sort downstream when it matters.
//!
//! Failure is `ExecError::Node` with the offending path in the message
//! (missing folder, a file path, unreadable entry, empty or non-string path).
//! A non-UTF-8 entry name is skipped with a warn log instead of aborting the
//! scan: `String` ports cannot carry it faithfully, and one odd file should
//! not hide the rest of the folder.

use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::executor::{EvalCtx, ExecError, NodeInputs, NodeOutputs};

pub fn exec(
    inputs: &NodeInputs<'_>,
    params: &Map<String, Value>,
    ctx: &mut EvalCtx<'_>,
) -> Result<NodeOutputs, ExecError> {
    let raw = inputs
        .get("path")
        .copied()
        .or_else(|| params.get("path"))
        .unwrap_or(&Value::Null);

    // Resolve before touching disk: a missing or non-string path is a user
    // error to report, never a panic (the previous `unwrap()` aborted the
    // whole run with no node context).
    let pathstr = match raw.as_str() {
        Some(s) if !s.trim().is_empty() => s.trim(),
        Some(_) => return Err(node_error("path is empty")),
        None => return Err(node_error("path must be a string")),
    };

    let entries = std::fs::read_dir(pathstr)
        .map_err(|e| node_error(format!("could not read '{pathstr}': {e}")))?;

    let mut filenames: Vec<Value> = Vec::new();
    let mut paths: Vec<Value> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| node_error(format!("could not read '{pathstr}': {e}")))?;
        let name = entry.file_name();
        let full = entry.path();
        match (name.to_str(), full.to_str()) {
            (Some(name), Some(full)) => {
                filenames.push(Value::String(name.to_string()));
                paths.push(Value::String(full.to_string()));
            }
            _ => ctx.warn(
                "file.iterate",
                format!("skipping non-UTF-8 entry in {pathstr}"),
            ),
        }
    }

    Ok(HashMap::from([
        ("filenames".to_string(), Value::Array(filenames)),
        ("paths".to_string(), Value::Array(paths)),
    ]))
}

fn node_error(message: impl Into<String>) -> ExecError {
    ExecError::Node {
        node: "file.iterate".into(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;

    use serde_json::{json, Map, Value};

    use super::exec;
    use crate::executor::nodes::test_helpers::{ctx_with, unique_tmp};
    use crate::executor::ExecError;

    /// RAII temp directory: a folder to scan, removed on drop. `TmpFile`
    /// only unlinks files, and these tests need a directory of their own.
    struct TmpDir(std::path::PathBuf);

    impl TmpDir {
        fn new(label: &str) -> Self {
            let dir = unique_tmp(label).with_extension("dir");
            fs::create_dir_all(&dir).expect("create temp dir");
            TmpDir(dir)
        }
    }

    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Run the node and return `(filenames, paths)`, each sorted so the
    /// assertions do not depend on `read_dir`'s unspecified order.
    fn run(
        params: Value,
        input_path: Option<&str>,
    ) -> Result<(Vec<String>, Vec<String>), ExecError> {
        let params: Map<String, Value> = params.as_object().unwrap().clone();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let wired = input_path.map(|p| Value::String(p.to_string()));
        let mut inputs: HashMap<String, &Value> = HashMap::new();
        if let Some(value) = wired.as_ref() {
            inputs.insert("path".to_string(), value);
        }
        let mut ctx = ctx_with("n1", "file.iterate", &external, &mut outs, &mut logs);
        let out = exec(&inputs, &params, &mut ctx)?;
        Ok((strings(out.get("filenames")), strings(out.get("paths"))))
    }

    fn strings(value: Option<&Value>) -> Vec<String> {
        let mut out: Vec<String> = value
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| item.as_str())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        out.sort();
        out
    }

    #[test]
    fn lists_files_and_full_paths() {
        let dir = TmpDir::new("iterate_basic");
        fs::write(dir.0.join("alpha.txt"), "a").unwrap();
        fs::write(dir.0.join("beta.txt"), "b").unwrap();
        let root = dir.0.display().to_string();

        let (filenames, paths) = run(json!({ "path": root }), None).unwrap();

        assert_eq!(filenames, vec!["alpha.txt", "beta.txt"]);
        assert_eq!(paths.len(), 2);
        assert!(paths[0].ends_with("alpha.txt"));
        assert!(paths[1].ends_with("beta.txt"));
        assert!(paths.iter().all(|p| p.starts_with(&root)));
    }

    #[test]
    fn includes_subdirectories() {
        let dir = TmpDir::new("iterate_dirs");
        fs::create_dir(dir.0.join("nested")).unwrap();
        fs::write(dir.0.join("file.txt"), "x").unwrap();

        let (filenames, _) = run(json!({ "path": dir.0.display().to_string() }), None).unwrap();

        assert_eq!(filenames, vec!["file.txt", "nested"]);
    }

    #[test]
    fn empty_folder_yields_empty_arrays() {
        let dir = TmpDir::new("iterate_empty");

        let (filenames, paths) = run(json!({ "path": dir.0.display().to_string() }), None).unwrap();

        assert!(filenames.is_empty());
        assert!(paths.is_empty());
    }

    #[test]
    fn wired_input_overrides_param_path() {
        let dir = TmpDir::new("iterate_wired");
        fs::write(dir.0.join("only.txt"), "x").unwrap();
        let root = dir.0.display().to_string();

        let (filenames, paths) = run(
            json!({ "path": "/definitely/not/a/folder" }),
            Some(root.as_str()),
        )
        .unwrap();

        assert_eq!(filenames, vec!["only.txt"]);
        assert_eq!(paths.len(), 1);
    }

    #[test]
    fn errors_on_missing_folder() {
        let dir = TmpDir::new("iterate_missing");
        let missing = dir.0.join("nope").display().to_string();
        assert!(matches!(
            run(json!({ "path": missing }), None).unwrap_err(),
            ExecError::Node { .. }
        ));
    }

    /// Regression for the bug these tests used to encode: a *file* path
    /// reaching `read_dir` must surface as a node error, not a panic.
    #[test]
    fn errors_on_file_path() {
        let dir = TmpDir::new("iterate_file");
        let file = dir.0.join("a.txt");
        fs::write(&file, "x").unwrap();
        assert!(matches!(
            run(json!({ "path": file.display().to_string() }), None).unwrap_err(),
            ExecError::Node { .. }
        ));
    }

    #[test]
    fn errors_on_empty_path() {
        assert!(matches!(
            run(json!({ "path": "   " }), None).unwrap_err(),
            ExecError::Node { .. }
        ));
    }

    #[test]
    fn errors_when_path_is_not_a_string() {
        assert!(matches!(
            run(json!({ "path": 7 }), None).unwrap_err(),
            ExecError::Node { .. }
        ));
        assert!(matches!(
            run(json!({}), None).unwrap_err(),
            ExecError::Node { .. }
        ));
    }
}
