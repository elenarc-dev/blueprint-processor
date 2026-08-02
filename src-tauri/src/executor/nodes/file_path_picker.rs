//! `file.pathPicker` — passthrough node whose `params.path` is
//! populated by the BlueprintNode UI's "Browse…" button (which calls
//! `tauri-plugin-dialog`'s `open()` under the hood).
//!
//! **Why a passthrough at all?** The Rust executor exists so the node
//! is a first-class citizen in the runner — every other kind goes
//! through `exec(...)` and so does this one. The actual
//! path-selection work happens in the frontend (UI →
//! `@tauri-apps/plugin-dialog` → string|array returned, written
//! back to `params.path` via `updateParam`). The executor just
//! validates the shape and re-emits.
//!
//! **Shape mirroring** — input `path` (port type `string`) may
//! carry a scalar `Value::String` or a `Value::Array` thereof. The
//! output `path` mirrors the input shape 1:1:
//!   * `Value::String` in  → `Value::String` out
//!   * `Value::Array`  in  → `Value::Array` out (same length)
//!   * empty array in     → `Value::Array([])` out (no-op)
//!   * `Value::Null` / empty string → `ExecError::Node` (user
//!     hasn't picked anything; surface this loudly before
//!     downstream `file.read` etc. fail with a less-obvious error
//!     like "could not read ''").
//!
//! **Future-proofing:** the executor's "input port overrides param"
//! branch (`inputs.get("path")`) is currently unreachable because
//! the catalog declares `inputs: []` for this kind — the runner's
//! `build_incoming` only feeds declared ports. Kept in place so a
//! future kind that re-declares picker semantics with an input
//! port does not require a code change here.

use serde_json::{Map, Value};

use crate::executor::{single, EvalCtx, ExecError, NodeInputs, NodeOutputs};

pub fn exec(
    inputs: &NodeInputs<'_>,
    params: &Map<String, Value>,
    _ctx: &mut EvalCtx<'_>,
) -> Result<NodeOutputs, ExecError> {
    let param_path = params.get("path").unwrap_or(&Value::Null);
    let provided = inputs.get("path").copied().unwrap_or(param_path);

    match provided {
        // ---------- single path ----------
        Value::String(s) if !s.is_empty() => {
            Ok(single("path", Value::String(s.clone())))
        }

        // ---------- scalar null / empty ----------
        Value::String(_) => Err(ExecError::Node {
            node: "file.pathPicker".into(),
            message: "selected path is empty (click Browse to pick one)".into(),
        }),
        Value::Null => Err(ExecError::Node {
            node: "file.pathPicker".into(),
            message: "no path selected yet (click Browse to pick one)".into(),
        }),

        // ---------- array of paths ----------
        Value::Array(items) => {
            // Length is preserved even when the user opens the dialog
            // and the array is empty (e.g. clear button). Downstream
            // `file.read`/`file.saveBinary` handle empty arrays
            // symbolically without erroring, so we mirror that.
            let mut out: Vec<Value> = Vec::with_capacity(items.len());
            for (i, slot) in items.iter().enumerate() {
                let s = slot.as_str().ok_or_else(|| ExecError::Node {
                    node: "file.pathPicker".into(),
                    message: format!("`path[{i}]` must be a string"),
                })?;
                if s.is_empty() {
                    return Err(ExecError::Node {
                        node: "file.pathPicker".into(),
                        message: format!("`path[{i}]` is empty"),
                    });
                }
                out.push(Value::String(s.to_string()));
            }
            Ok(single("path", Value::Array(out)))
        }

        // ---------- non-string scalar (e.g. number, bool, object) ----------
        other => Err(ExecError::Node {
            node: "file.pathPicker".into(),
            message: format!(
                "`path` must be a string or array of strings (got {})",
                crate::executor::value_type_name(other)
            ),
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::{json, Value};

    use super::ExecError;
    use super::exec;
    use crate::executor::nodes::test_helpers::ctx_with;

    /// Spawn an executor and return just the `path` output value.
    /// Lower-level tests assert directly on `r.get("path")`; this
    /// helper is convenient for the bulk of pass-through tests.
    fn run(params: serde_json::Map<String, Value>) -> Result<Value, ExecError> {
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let inputs: HashMap<String, &Value> = HashMap::new();
        let mut ctx = ctx_with("n1", "file.pathPicker", &external, &mut outs, &mut logs);
        exec(&inputs, &params, &mut ctx).map(|m| m.get("path").cloned().unwrap())
    }

    // ---------- single path ----------

    #[test]
    fn emits_single_string_param_path() {
        let params = json!({ "mode": "file", "path": "/tmp/foo.png" })
            .as_object()
            .unwrap()
            .clone();
        let out = run(params).unwrap();
        assert_eq!(out.as_str(), Some("/tmp/foo.png"));
    }

    #[test]
    fn errors_on_empty_string() {
        let params = json!({ "path": "" }).as_object().unwrap().clone();
        let err = run(params).unwrap_err();
        assert!(
            matches!(err, ExecError::Node { ref message, .. } if message.contains("empty")
                || message.contains("Browse")),
            "expected empty or Browse message, got: {err:?}"
        );
    }

    #[test]
    fn errors_on_null_param_path() {
        let params = json!({}).as_object().unwrap().clone();
        let err = run(params).unwrap_err();
        assert!(
            matches!(err, ExecError::Node { ref message, .. } if message.contains("no path")
                || message.contains("Browse")),
            "expected no-path or Browse message, got: {err:?}"
        );
    }

    #[test]
    fn errors_on_non_string_scalar() {
        // A number / bool / object accidentally stored in params.path
        // bubbles up with a type-name so the user can find the bug.
        let params = json!({ "path": 42 }).as_object().unwrap().clone();
        let err = run(params).unwrap_err();
        assert!(matches!(err, ExecError::Node { .. }));
    }

    // ---------- array of paths ----------

    #[test]
    fn emits_array_of_paths() {
        let params = json!({ "mode": "multiple-files", "path": ["/a", "/b", "/c"] })
            .as_object()
            .unwrap()
            .clone();
        let out = run(params).unwrap();
        let arr = out.as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0].as_str(), Some("/a"));
        assert_eq!(arr[1].as_str(), Some("/b"));
        assert_eq!(arr[2].as_str(), Some("/c"));
    }

    #[test]
    fn emits_empty_array_as_no_op() {
        // The UI clears via an empty array (the "x" button), not by
        // removing params.path. We treat empty-array as a legitimate
        // no-op — the user's downstream graph might want to do
        // nothing this run rather than error.
        let params = json!({ "path": [] }).as_object().unwrap().clone();
        let out = run(params).unwrap();
        assert!(out.as_array().unwrap().is_empty());
    }

    #[test]
    fn errors_on_array_with_empty_string_slot() {
        // One empty string in the array — we keep the unary "every
        // path must be non-empty" invariant. Otherwise downstream
        // `file.read` would fail with a confusing "could not read ''"
        // rather than this cleaner "path[i] is empty" message.
        let params = json!({ "path": ["/a", "", "/c"] })
            .as_object()
            .unwrap()
            .clone();
        let err = run(params).unwrap_err();
        assert!(
            matches!(err, ExecError::Node { ref message, .. } if message.contains("path[1]")),
            "expected message pointing at `path[1]`, got: {err:?}"
        );
    }

    #[test]
    fn errors_on_array_with_non_string_slot() {
        let params = json!({ "path": ["/a", 42] })
            .as_object()
            .unwrap()
            .clone();
        let err = run(params).unwrap_err();
        assert!(
            matches!(err, ExecError::Node { ref message, .. } if message.contains("path[1]")),
            "expected message pointing at `path[1]`, got: {err:?}"
        );
    }

    // ---------- connected input port overrides params.path ----------
    // These two tests cover a code path that's currently DEAD (the
    // picker has no declared `path` input port) but is structurally
    // part of the executor's port-vs-param precedence rule. They
    // document the intent so a future catalogue change adding an
    // input port doesn't quietly break the contract.

    #[test]
    fn connected_path_port_overrides_param_for_scalar() {
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let from_port = Value::String("/from-port.png".into());
        let mut inputs = HashMap::new();
        // `.to_string()` so the key is `String` (not `&str`) — matches
        // the `NodeInputs<'a> = HashMap<String, &'a Value>` signature.
        inputs.insert("path".to_string(), &from_port);
        let params = json!({ "path": "/from-param.png" })
            .as_object()
            .unwrap()
            .clone();
        let mut ctx = ctx_with(
            "n1",
            "file.pathPicker",
            &external,
            &mut outs,
            &mut logs,
        );
        let out = exec(&inputs, &params, &mut ctx).unwrap();
        assert_eq!(out.get("path").unwrap().as_str(), Some("/from-port.png"));
    }

    #[test]
    fn connected_path_port_overrides_param_for_array() {
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let from_port = Value::Array(vec![
            Value::String("/x".into()),
            Value::String("/y".into()),
        ]);
        let mut inputs = HashMap::new();
        // `.to_string()` so the key is `String` (not `&str`) — matches
        // the `NodeInputs<'a> = HashMap<String, &'a Value>` signature.
        inputs.insert("path".to_string(), &from_port);
        let params = json!({ "path": ["/legacy-a", "/legacy-b"] })
            .as_object()
            .unwrap()
            .clone();
        let mut ctx = ctx_with(
            "n1",
            "file.pathPicker",
            &external,
            &mut outs,
            &mut logs,
        );
        let out = exec(&inputs, &params, &mut ctx).unwrap();
        let arr = out.get("path").unwrap().as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0].as_str(), Some("/x"));
        assert_eq!(arr[1].as_str(), Some("/y"));
    }
}
