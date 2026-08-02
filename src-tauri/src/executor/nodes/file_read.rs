//! `file.read` — reads one (or many) files from disk.
//!
//! Three output ports per read:
//! - `text`   — full content as a string (UTF-8 with `from_utf8_lossy`
//!              fallback; non-UTF-8 bytes morph into U+FFFD
//!              replacement chars so the user can SEE the corruption
//!              instead of getting a silent empty string).
//! - `lines`  — per-line array<string>. `file.read` was originally a
//!              text-only node and we keep the per-line convenience
//!              for the single-file branch. For the array-of-paths
//!              branch we drop per-line emission (no obvious shape
//!              for an array of per-line arrays).
//! - `binary` — base64-encoded raw bytes. Replaces the prior
//!              standalone `file.readBinary` node (deleted) so the
//!              user does not have to choose between two near-identical
//!              catalog entries for the same I/O pattern.
//!
//! All three ports are emitted in a single pass: we read the file
//! once into `Vec<u8>`, then derive the string and base64 views from
//! that buffer. This avoids a redundant `read_to_string` followed by
//! a `read` of the same path on disk. It also fixes a latent bug in
//! the prior implementation where `.map_err(...).unwrap_or("")`
//! silently swallowed missing-file errors and returned
//! `Ok(empty_outputs)` — propagated via `?` here.

use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::executor::{encode_b64, EvalCtx, ExecError, NodeInputs, NodeOutputs};

pub fn exec(
    _inputs: &NodeInputs<'_>,
    params: &Map<String, Value>,
    _ctx: &mut EvalCtx<'_>,
) -> Result<NodeOutputs, ExecError> {
    let param_path = params.get("path").unwrap_or(&Value::Null);
    let path = _inputs.get("path").copied().unwrap_or(param_path);

    if path.eq(&Value::Null) {
        return Err(ExecError::Node {
            node: "file.read".into(),
            message: "params.path is empty".into(),
        });
    }

    match path {
        // ---------- single file ----------
        Value::String(s) => {
            let path_str = s.as_str().to_string();
            // Read once. `read_to_string` and `read` would normally
            // hit disk twice on the SAME path; that's wasteful and
            // racy if the file mutates between reads. Single `read`
            // gives us both views from the same buffer.
            let bytes = std::fs::read(&path_str).map_err(|e| ExecError::Node {
                node: "file.read".into(),
                message: format!("could not read '{path_str}': {e}"),
            })?;

            // UTF-8 view: lossy on non-UTF-8 bytes (replaces them
            // with U+FFFD). Lines are derived from the *lossy*
            // string so they reflect what the user sees on the `text`
            // port — a replacement-character line is the honest
            // representation when the source had stray bytes.
            let text = String::from_utf8_lossy(&bytes).into_owned();
            let lines: Vec<Value> = text
                .split('\n')
                .map(|line| line.trim_end_matches('\r').to_string())
                .map(Value::String)
                .collect();

            let mut outputs: HashMap<String, Value> = HashMap::with_capacity(3);
            outputs.insert("text".into(), Value::String(text));
            outputs.insert("lines".into(), Value::Array(lines));
            outputs.insert("binary".into(), Value::String(encode_b64(&bytes)));
            Ok(outputs)
        }

        // ---------- array of files ----------
        Value::Array(items) => {
            let mut texts: Vec<Value> = Vec::with_capacity(items.len());
            let mut binaries: Vec<Value> = Vec::with_capacity(items.len());
            for slot in items {
                let p = slot.as_str().ok_or_else(|| ExecError::Node {
                    node: "file.read".into(),
                    message: format!("array slot {slot} is not a string path"),
                })?;
                let bytes = std::fs::read(p).map_err(|e| ExecError::Node {
                    node: "file.read".into(),
                    message: format!("could not read '{p}': {e}"),
                })?;
                let text = String::from_utf8_lossy(&bytes).into_owned();
                texts.push(Value::String(text));
                binaries.push(Value::String(encode_b64(&bytes)));
            }
            let mut outputs: HashMap<String, Value> = HashMap::with_capacity(2);
            outputs.insert("text".into(), Value::Array(texts));
            outputs.insert("binary".into(), Value::Array(binaries));
            Ok(outputs)
        }

        _ => Err(ExecError::Node {
            node: "file.read".into(),
            message: "path must be a string or array of strings".into(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;

    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine as _;
    use serde_json::{json, Value};

    use super::exec;
    use crate::executor::nodes::test_helpers::{ctx_with, TmpFile};

    fn run_path(path: String) -> HashMap<String, Value> {
        let params = json!({ "path": path }).as_object().unwrap().clone();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let inputs = HashMap::new();
        let mut ctx = ctx_with("n1", "file.read", &external, &mut outs, &mut logs);
        exec(&inputs, &params, &mut ctx).unwrap()
    }

    fn decode_b64(s: &str) -> Vec<u8> {
        B64.decode(s).expect("base64 decode")
    }

    #[test]
    fn reads_text_and_lines() {
        let file = TmpFile::new("read_basic");
        let path = file.0.to_string_lossy().to_string();
        let body = "alpha\nbeta\ngamma";
        fs::write(&path, body).unwrap();
        let r = run_path(path);

        assert_eq!(r.get("text").unwrap().as_str(), Some(body));
        let arr = r.get("lines").unwrap().as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0].as_str(), Some("alpha"));
        assert_eq!(arr[1].as_str(), Some("beta"));
        assert_eq!(arr[2].as_str(), Some("gamma"));
        // The binary port is also emitted; round-trip it through the
        // same encoder so we know the bytes that hit disk survive.
        let bytes = decode_b64(r.get("binary").unwrap().as_str().unwrap());
        assert_eq!(bytes, body.as_bytes());
    }

    #[test]
    fn handles_crlf_line_endings() {
        let file = TmpFile::new("read_crlf");
        let path = file.0.to_string_lossy().to_string();
        let body = "first\r\nsecond\r\nthird";
        fs::write(&path, body).unwrap();
        let r = run_path(path);

        // 1) The `lines` port strips CR (catalog contract).
        let arr = r.get("lines").unwrap().as_array().unwrap();
        assert_eq!(arr.len(), 3);
        for v in arr {
            assert!(!v.as_str().unwrap().contains('\r'));
        }
        // 2) The `text` port KEEPS CR — the lossy view reflects the
        //    byte stream faithfully and only `lines` strips the
        //    carriage returns for visual line-by-line use. Pinned
        //    here so a future refactor doesn't "normalise" text.
        let text = r.get("text").unwrap().as_str().unwrap();
        assert!(text.contains('\r'), "text must preserve CR bytes");
        assert_eq!(text, "first\r\nsecond\r\nthird");
        // 3) The `binary` port emits the literal byte stream.
        let bytes = decode_b64(r.get("binary").unwrap().as_str().unwrap());
        assert_eq!(bytes, body.as_bytes());
    }

    #[test]
    fn reads_empty_file() {
        let file = TmpFile::new("read_empty");
        let path = file.0.to_string_lossy().to_string();
        fs::write(&path, "").unwrap();
        let r = run_path(path);
        assert_eq!(r.get("text").unwrap().as_str(), Some(""));
        assert_eq!(
            r.get("lines").unwrap().as_array().unwrap().len(),
            1, // split("") yields [""]
        );
        assert_eq!(
            decode_b64(r.get("binary").unwrap().as_str().unwrap()),
            Vec::<u8>::new()
        );
    }

    #[test]
    fn binary_port_round_trips_arbitrary_bytes() {
        // The whole point of the binary port: the user can pipe
        // non-UTF-8 bytes through the pipeline without `file.read`
        // erroring or corrupting. Verifying with random-but-valid
        // byte values around the 0xC0/0x80 continuation boundary.
        let file = TmpFile::new("read_binary");
        let path = file.0.to_string_lossy().to_string();
        let payload: &[u8] = &[0xFF, 0xD8, 0x00, 0x10, b'h', b'i', 0xAB, 0xCD];
        fs::write(&path, payload).unwrap();
        let r = run_path(path);

        let round = decode_b64(r.get("binary").unwrap().as_str().unwrap());
        assert_eq!(round, payload);
    }

    #[test]
    fn text_port_inserts_replacement_char_for_invalid_utf8() {
        // The previous behaviour silently returned empty text on
        // non-UTF-8 input. The new behaviour keeps the byte stream
        // honest via `from_utf8_lossy` — invalid bytes morph into
        // U+FFFD so the user can SEE corruption rather than have it
        // hidden. Pin this so a future "fix" doesn't undo the change.
        let file = TmpFile::new("read_invalid_utf8");
        let path = file.0.to_string_lossy().to_string();
        fs::write(&path, [0xFFu8]).unwrap();
        let r = run_path(path);

        assert_eq!(r.get("text").unwrap().as_str(), Some("\u{FFFD}"));
        let bins = decode_b64(r.get("binary").unwrap().as_str().unwrap());
        assert_eq!(bins, vec![0xFFu8]);
    }

    #[test]
    fn reads_array_of_paths() {
        let f1 = TmpFile::new("read_arr1");
        let f2 = TmpFile::new("read_arr2");
        let p1 = f1.0.to_string_lossy().to_string();
        let p2 = f2.0.to_string_lossy().to_string();
        fs::write(&p1, b"alpha").unwrap();
        fs::write(&p2, b"beta").unwrap();
        let params = json!({ "path": [p1.clone(), p2.clone()] })
            .as_object()
            .unwrap()
            .clone();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let inputs = HashMap::new();
        let mut ctx = ctx_with("n1", "file.read", &external, &mut outs, &mut logs);
        let r = exec(&inputs, &params, &mut ctx).unwrap();

        let texts = r.get("text").unwrap().as_array().unwrap();
        assert_eq!(texts.len(), 2);
        assert_eq!(texts[0].as_str(), Some("alpha"));
        assert_eq!(texts[1].as_str(), Some("beta"));

        let bins = r.get("binary").unwrap().as_array().unwrap();
        assert_eq!(bins.len(), 2);
        assert_eq!(decode_b64(bins[0].as_str().unwrap()), b"alpha");
        assert_eq!(decode_b64(bins[1].as_str().unwrap()), b"beta");
    }

    #[test]
    fn reads_empty_array() {
        // Pinned so a future contributor doesn't "optimise" the
        // empty-array branch back to emitting only `text` — the
        // current shape is symmetric (text and binary arrays), and
        // any downstream consumer that connects one of the two
        // arrays expects the parallel one to also exist.
        let params = json!({ "path": Vec::<String>::new() })
            .as_object()
            .unwrap()
            .clone();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let inputs = HashMap::new();
        let mut ctx = ctx_with("n1", "file.read", &external, &mut outs, &mut logs);
        let r = exec(&inputs, &params, &mut ctx).unwrap();

        assert!(r.get("text").unwrap().as_array().unwrap().is_empty());
        assert!(r.get("binary").unwrap().as_array().unwrap().is_empty());
    }

    #[test]
    fn errors_on_missing_file() {
        let file = TmpFile::new("read_missing");
        let path = file.0.to_string_lossy().to_string();
        let params = json!({ "path": path }).as_object().unwrap().clone();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let inputs = HashMap::new();
        let mut ctx = ctx_with("n1", "file.read", &external, &mut outs, &mut logs);
        let err = exec(&inputs, &params, &mut ctx).unwrap_err();
        assert!(matches!(err, crate::executor::ExecError::Node { .. }));
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
            crate::executor::ExecError::Node { .. }
        ));
    }

    #[test]
    fn errors_on_non_string_array_slot() {
        let params = json!({ "path": ["/never/used", 42] })
            .as_object()
            .unwrap()
            .clone();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let inputs = HashMap::new();
        let mut ctx = ctx_with("n1", "file.read", &external, &mut outs, &mut logs);
        let err = exec(&inputs, &params, &mut ctx).unwrap_err();
        assert!(matches!(err, crate::executor::ExecError::Node { .. }));
    }
}
