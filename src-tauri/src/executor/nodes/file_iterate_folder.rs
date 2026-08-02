//! `file.read` — reads a UTF-8 text file from disk.
//! Emits BOTH the full content (`text`, string) and a per-line array
//! (`lines`, array<string>) — saving the user a `string.split` step.
//! CRLF line endings are normalised to LF on the `lines` port; the
//! `text` port keeps the original characters.

use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::executor::{EvalCtx, ExecError, NodeInputs, NodeOutputs};

pub fn exec(
    _inputs: &NodeInputs<'_>,
    params: &Map<String, Value>,
    _ctx: &mut EvalCtx<'_>,
) -> Result<NodeOutputs, ExecError> {
    let param_path = params.get("path").unwrap_or(&Value::Null);
    let path = _inputs.get("path").copied().unwrap_or(param_path);

    let pathstr = path.as_str().unwrap();
    if pathstr.is_empty() {
        return Err(ExecError::Node {
            node: "file.iterate".into(),
            message: "params.path is empty".into(),
        });
    }

    let content = std::fs::read_dir(&pathstr).map_err(|e| ExecError::Node {
        node: "file.iterate".into(),
        message: format!("could not read '{path}': {e}"),
    })?;

    let (files, routes): (Vec<Value>, Vec<Value>) = content
        .map(|x| {
            (
                Value::String(String::from(
                    x.as_ref().unwrap().file_name().to_str().unwrap(),
                )),
                Value::String(String::from(x.as_ref().unwrap().path().to_str().unwrap())),
            )
        })
        .collect();

    Ok(HashMap::from([
        ("filenames".to_string(), Value::Array(files)),
        ("paths".to_string(), Value::Array(routes)),
    ]))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;

    use serde_json::{json, Value};

    use super::exec;
    use super::ExecError;
    use crate::executor::nodes::test_helpers::{ctx_with, TmpFile};

    fn run(path: String) -> HashMap<String, Value> {
        let params = json!({ "path": path }).as_object().unwrap().clone();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let inputs = HashMap::new();
        let mut ctx = ctx_with("n1", "file.read", &external, &mut outs, &mut logs);
        exec(&inputs, &params, &mut ctx).unwrap()
    }

    #[test]
    fn reads_text_and_lines() {
        let file = TmpFile::new("read_basic");
        let path = file.0.to_string_lossy().to_string();
        let body = "alpha\nbeta\ngamma";
        fs::write(&path, body).unwrap();
        let r = run(path);

        assert_eq!(r.get("text").unwrap().as_str(), Some(body));
        let arr = r.get("lines").unwrap().as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0].as_str(), Some("alpha"));
        assert_eq!(arr[1].as_str(), Some("beta"));
        assert_eq!(arr[2].as_str(), Some("gamma"));
        // file guard drops here.
    }

    #[test]
    fn handles_crlf_line_endings() {
        let file = TmpFile::new("read_crlf");
        let path = file.0.to_string_lossy().to_string();
        let body = "first\r\nsecond\r\nthird";
        fs::write(&path, body).unwrap();
        let r = run(path);

        let arr = r.get("lines").unwrap().as_array().unwrap();
        // CRLF normalised to LF → 3 lines, no \r.
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0].as_str(), Some("first"));
        assert_eq!(arr[1].as_str(), Some("second"));
        assert_eq!(arr[2].as_str(), Some("third"));
        for v in arr {
            assert!(!v.as_str().unwrap().contains('\r'));
        }
    }

    #[test]
    fn reads_empty_file() {
        let file = TmpFile::new("read_empty");
        let path = file.0.to_string_lossy().to_string();
        fs::write(&path, "").unwrap();
        let r = run(path);
        assert_eq!(r.get("text").unwrap().as_str(), Some(""));
        assert_eq!(
            r.get("lines").unwrap().as_array().unwrap().len(),
            1, // split("") yields [""] — single empty line — matches Python/Ruby semantics
        );
    }

    #[test]
    fn errors_on_missing_file() {
        // Path is allocated but never written — `read_to_string` errors.
        let file = TmpFile::new("read_missing");
        let path = file.0.to_string_lossy().to_string();
        let params = json!({ "path": path }).as_object().unwrap().clone();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let inputs = HashMap::new();
        let mut ctx = ctx_with("n1", "file.read", &external, &mut outs, &mut logs);
        let err = exec(&inputs, &params, &mut ctx).unwrap_err();
        assert!(matches!(err, ExecError::Node { .. }));
    }

    #[test]
    fn errors_on_empty_path() {
        let params = json!({ "path": "" }).as_object().unwrap().clone();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let inputs = HashMap::new();
        let mut ctx = ctx_with("n1", "file.read", &external, &mut outs, &mut logs);
        assert!(matches!(
            exec(&inputs, &params, &mut ctx).unwrap_err(),
            ExecError::Node { .. }
        ));
    }
}
