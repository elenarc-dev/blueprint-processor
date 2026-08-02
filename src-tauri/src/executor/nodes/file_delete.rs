//! `file.delete` — remove one or more paths from disk.
//!
//! Inputs and params follow the codebase's scalar-or-array convention:
//!   * `Value::String` (non-empty) at either an `inputs.paths` port or
//!     `params.paths` → single delete (and a multi-line string is split
//!     on `\n` into an implicit array — see `delete_from_string`).
//!   * `Value::Array` of strings — one delete per slot.
//!   * Empty array  → no-op (legitimate "cleared" case; mirrors
//!     `file.pathPicker`'s contract for downstream `file.iterate`
//!     users).
//!   * Empty string / null / wrong-type → `ExecError::Node`.
//!
//! The `deleted` output mirrors the input shape so downstream nodes
//! (logger, file.move, etc.) can chain without re-resolving the list.
//!
//! **Params:**
//!   * `recursive` (boolean, default `false`) — when `true` uses
//!     `fs::remove_dir_all` (works on files AND directories; wipes a
//!     folder tree). When `false` uses `fs::remove_file` (files only,
//!     errors cleanly on directories — the safe default that keeps a
//!     misclicked dropdown from `rm -rf` ing a folder).
//!
//! **Error reporting:** every IO error and every shape mismatch is
//! wrapped as `ExecError::Node` with the offending path string in
//! `message`. Array slots report the index (`paths[3] is empty`) so
//! the user can pinpoint the bad row without reading the path values.
//!
//! **Port-vs-param precedence.** The catalog declares
//! `inputs: [{ id: "paths", ... }]`, so the runner feeds the input
//! port when wired. `inputs.paths` overrides `params.paths` — same
//! pattern as `file.pathPicker`.

use serde_json::{Map, Value};

use crate::executor::{single, EvalCtx, ExecError, NodeInputs, NodeOutputs};

pub fn exec(
    inputs: &NodeInputs<'_>,
    params: &Map<String, Value>,
    _ctx: &mut EvalCtx<'_>,
) -> Result<NodeOutputs, ExecError> {
    let recursive = params
        .get("recursive")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // Port-vs-param precedence. `NodeInputs<'a> = HashMap<String,
    // &'a Value>` so `.get(...)` yields `Option<&&Value>`. We can't
    // use `Option::cloned()` here: it only unwraps ONE level of
    // reference (because `Clone for &T` returns `&T`), so on
    // `Option<&&Value>` it returns `Option<&Value>`, not
    // `Option<Value>`. We map-then-deref instead.
    let provided: Value = inputs
        .get("paths")
        .map(|v| (*v).clone())
        .or_else(|| params.get("paths").cloned())
        .unwrap_or(Value::Null);

    match provided {
        Value::String(s) => delete_from_string(&s, recursive),
        Value::Array(items) => delete_array(&items, recursive),
        Value::Null => Err(ExecError::Node {
            node: "file.delete".into(),
            message: "no path provided (connect `paths` or set params.paths)".into(),
        }),
        other => Err(ExecError::Node {
            node: "file.delete".into(),
            message: format!(
                "`paths` must be a string or array of strings (got {})",
                crate::executor::value_type_name(&other)
            ),
        }),
    }
}

/// Delete the paths derived from a `Value::String`. The body's
/// textarea editor stores one path per line, so a multi-line
/// `params.paths` value must be split at run-time — the executor is
/// the canonical place for that split because it round-trips through
/// persisted blueprints (a saved JSON file with `"paths": "a\nb\nc"`
/// should still delete all three on next run, with no UI-level
/// transformation in the loop).
///
/// Semantics:
///   * `""`                     → "no non-blank lines" error
///   * single-line (1 path)     → `Value::String` out (scalar echoing)
///   * multi-line (≥2 non-blank) → `Value::Array` out (parallel echoing)
fn delete_from_string(s: &str, recursive: bool) -> Result<NodeOutputs, ExecError> {
    let lines: Vec<&str> = s
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return Err(ExecError::Node {
            node: "file.delete".into(),
            message: "selected path is empty (only blank lines after trim)".into(),
        });
    }
    if lines.len() == 1 {
        delete_one(lines[0], recursive)?;
        return Ok(single("deleted", Value::String(lines[0].to_string())));
    }
    let mut out: Vec<Value> = Vec::with_capacity(lines.len());
    for p in &lines {
        delete_one(p, recursive)?;
        out.push(Value::String((*p).to_string()));
    }
    Ok(single("deleted", Value::Array(out)))
}

/// Delete every path in an array. Empty array is a no-op (mirrors
/// `file.pathPicker`'s contract — when `file.iterate` produces an
/// empty folder scan, the downstream delete should also be empty).
fn delete_array(items: &[Value], recursive: bool) -> Result<NodeOutputs, ExecError> {
    let mut out: Vec<Value> = Vec::with_capacity(items.len());
    for (i, slot) in items.iter().enumerate() {
        let s = slot.as_str().ok_or_else(|| ExecError::Node {
            node: "file.delete".into(),
            message: format!("`paths[{i}]` must be a string"),
        })?;
        if s.is_empty() {
            return Err(ExecError::Node {
                node: "file.delete".into(),
                message: format!("`paths[{i}]` is empty"),
            });
        }
        delete_one(s, recursive)?;
        out.push(Value::String(s.to_string()));
    }
    Ok(single("deleted", Value::Array(out)))
}

/// Delete a single path, dispatching by `recursive` and the path's
/// filesystem kind.
///
/// Cross-platform quirk we work around: `std::fs::remove_dir_all`
/// calls `RemoveDirectoryW` on Windows which **errors on regular
/// files** ("El nombre del directorio no es válido" / os error 267),
/// even though it works fine on Linux/macOS. The naive
/// `if recursive { remove_dir_all } else { remove_file }` therefore
/// trips on `recursive_still_deletes_a_single_file` when CI runs on
/// Windows. We dispatch by `Path::is_dir()` instead:
///
///   * Known directories  → `remove_dir_all` only when `recursive=true`
///     (else we surface a friendly "is a directory; toggle recursive"
///     error so the user knows what to do).
///   * Everything else (files, missing paths, symlinks, sockets) →
///     `remove_file`. This is a no-op for unknown paths other than
///     surfacing the OS's "no such file or directory" IO error,
///     which is the user-helpful failure mode.
///
/// The `recursive` flag has no effect on file-only paths — it's a
/// directory-only switch.
fn delete_one(path: &str, recursive: bool) -> Result<(), ExecError> {
    let p = std::path::Path::new(path);
    let res = if p.is_dir() {
        if recursive {
            std::fs::remove_dir_all(p)
        } else {
            return Err(ExecError::Node {
                node: "file.delete".into(),
                message: format!(
                    "could not delete '{path}': path is a directory; set recursive=true"
                ),
            });
        }
    } else {
        std::fs::remove_file(p)
    };
    res.map_err(|e| ExecError::Node {
        node: "file.delete".into(),
        message: format!("could not delete '{path}': {e}"),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;

    use serde_json::{json, Value};

    use super::exec;
    use super::ExecError;
    use crate::executor::nodes::test_helpers::{ctx_with, TmpFile};

    /// Allocate a fresh guarded directory under the system temp dir.
    /// Not covered by `TmpFile` because the RAII guard only handles
    /// files — a deliberate, conscious choice (existing filesystem
    /// tests don't blur into directories).
    fn unique_tmp_dir(label: &str) -> std::path::PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let pid = std::process::id();
        std::env::temp_dir().join(format!("bp-test-{label}-{pid}-{nanos}"))
    }

    /// Spawn an executor and return just the `deleted` output value.
    /// Lower-level tests can inspect the full `m.get("deleted")` if
    /// they need richer assertions.
    fn run(params: serde_json::Map<String, Value>) -> Result<Value, ExecError> {
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let inputs: HashMap<String, &Value> = HashMap::new();
        let mut ctx = ctx_with("n1", "file.delete", &external, &mut outs, &mut logs);
        exec(&inputs, &params, &mut ctx).map(|m| m.get("deleted").cloned().unwrap())
    }

    // ---------- single file ----------

    #[test]
    fn deletes_single_file() {
        let file = TmpFile::new("delete_basic");
        fs::write(&file.0, "bye").unwrap();
        let path = file.0.to_string_lossy().to_string();

        let params = json!({ "paths": path }).as_object().unwrap().clone();
        let out = run(params).unwrap();
        assert_eq!(out.as_str(), Some(path.as_str()));
        assert!(!file.0.exists(), "file should have been removed");
    }

    #[test]
    fn errors_on_empty_string() {
        let params = json!({ "paths": "" }).as_object().unwrap().clone();
        let err = run(params).unwrap_err();
        assert!(
            matches!(err, ExecError::Node { ref message, .. }
                if message.contains("empty") || message.contains("blank")),
            "expected empty / blank-line message, got: {err:?}"
        );
    }

    #[test]
    fn errors_on_whitespace_only_string() {
        // Pure whitespace + newlines (no real path) is the same as
        // empty from the executor's perspective. Saves the user from
        // a confusing "could not delete '\\n\\n'" IO error.
        let params = json!({ "paths": "   \n   \n" }).as_object().unwrap().clone();
        let err = run(params).unwrap_err();
        assert!(matches!(err, ExecError::Node { ref message, .. }
            if message.contains("empty")), "got: {err:?}");
    }

    #[test]
    fn errors_on_null_param_path() {
        let params = json!({}).as_object().unwrap().clone();
        let err = run(params).unwrap_err();
        assert!(
            matches!(err, ExecError::Node { ref message, .. }
                if message.contains("no path")),
            "expected no-path message, got: {err:?}"
        );
    }

    #[test]
    fn errors_on_non_string_scalar() {
        let params = json!({ "paths": 42 }).as_object().unwrap().clone();
        let err = run(params).unwrap_err();
        assert!(
            matches!(err, ExecError::Node { ref message, .. }
                if message.contains("string or array")),
            "expected type-mismatch message, got: {err:?}"
        );
    }

    #[test]
    fn errors_on_missing_file_default() {
        // Path allocated but never written → remove_file errors with
        // "The system cannot find the file specified" / "No such file
        // or directory" depending on the OS. Either is fine; we just
        // need an `ExecError::Node` so the runner halts the flow.
        let file = TmpFile::new("delete_missing");
        let path = file.0.to_string_lossy().to_string();
        let params = json!({ "paths": path }).as_object().unwrap().clone();
        let err = run(params).unwrap_err();
        assert!(
            matches!(err, ExecError::Node { ref message, .. }
                if message.contains("could not delete")),
            "expected could-not-delete message, got: {err:?}"
        );
    }

    // ---------- multi-line string (textarea path) ----------

    #[test]
    fn multi_line_string_splits_into_array() {
        // Regression for the bug the FIRST review caught: typing
        // multiple paths in the textarea stored them as a literal
        // newline-joined string which the executor used to try to
        // delete as ONE path. With the fix, the executor splits on
        // `\n` at run-time and emits a parallel array.
        let a = TmpFile::new("multiline_a");
        let b = TmpFile::new("multiline_b");
        fs::write(&a.0, "x").unwrap();
        fs::write(&b.0, "y").unwrap();
        let pa = a.0.to_string_lossy().to_string();
        let pb = b.0.to_string_lossy().to_string();
        let joined = format!("{pa}\n{pb}\n");

        let params = json!({ "paths": joined }).as_object().unwrap().clone();
        let out = run(params).unwrap();
        let arr = out.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0].as_str(), Some(pa.as_str()));
        assert_eq!(arr[1].as_str(), Some(pb.as_str()));
        assert!(!a.0.exists());
        assert!(!b.0.exists());
    }

    #[test]
    fn multi_line_strips_blank_lines_and_trims_whitespace() {
        // Empty lines and leading/trailing whitespace should NOT
        // empty out the array. Trim each line; drop empties. Note
        // the user-facing contract: the `deleted` output AND the
        // deletion itself both see the TRIMMED value. If the user
        // wires `file.delete.deleted → file.write.content`, the
        // write sees an un-padded path (different from a wired-port
        // path which is echoed verbatim; see catalog description).
        let a = TmpFile::new("multiline_trim_a");
        let b = TmpFile::new("multiline_trim_b");
        fs::write(&a.0, "x").unwrap();
        fs::write(&b.0, "y").unwrap();
        let pa = a.0.to_string_lossy().to_string();
        let pb = b.0.to_string_lossy().to_string();
        let joined = format!("  {pa}  \n\n  {pb}  \n\n");

        let params = json!({ "paths": joined }).as_object().unwrap().clone();
        let out = run(params).unwrap();
        let arr = out.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0].as_str(), Some(pa.trim()));
        assert_eq!(arr[1].as_str(), Some(pb.trim()));
    }

    // ---------- array of paths ----------

    #[test]
    fn deletes_array_of_files() {
        let a = TmpFile::new("delete_array_a");
        let b = TmpFile::new("delete_array_b");
        fs::write(&a.0, "x").unwrap();
        fs::write(&b.0, "y").unwrap();
        let pa = a.0.to_string_lossy().to_string();
        let pb = b.0.to_string_lossy().to_string();

        let params = json!({ "paths": [pa.clone(), pb.clone()] })
            .as_object()
            .unwrap()
            .clone();
        let out = run(params).unwrap();
        let arr = out.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0].as_str(), Some(pa.as_str()));
        assert_eq!(arr[1].as_str(), Some(pb.as_str()));
        assert!(!a.0.exists());
        assert!(!b.0.exists());
    }

    #[test]
    fn empty_array_is_no_op() {
        let params = json!({ "paths": [] }).as_object().unwrap().clone();
        let out = run(params).unwrap();
        assert!(out.as_array().unwrap().is_empty());
    }

    #[test]
    fn errors_on_array_with_empty_string_slot() {
        let file = TmpFile::new("delete_array_empty_slot");
        fs::write(&file.0, "").unwrap();
        let pa = file.0.to_string_lossy().to_string();
        let params = json!({ "paths": [pa, ""] })
            .as_object()
            .unwrap()
            .clone();
        let err = run(params).unwrap_err();
        assert!(
            matches!(err, ExecError::Node { ref message, .. } if message.contains("paths[1]")),
            "expected message pointing at `paths[1]`, got: {err:?}"
        );
        // Side effect: slot 0 was already deleted before the error fires.
        assert!(!file.0.exists());
    }

    #[test]
    fn errors_on_array_with_non_string_slot() {
        // Slot [0] is a real file (so the executor reaches slot [1]).
        // The previous version used "/x" which doesn't exist and the
        // missing-file IO error fired BEFORE the type mismatch —
        // the test was checking the wrong thing.
        let file = TmpFile::new("delete_array_non_string");
        fs::write(&file.0, "x").unwrap();
        let pa = file.0.to_string_lossy().to_string();
        let params = json!({ "paths": [pa, 42] })
            .as_object()
            .unwrap()
            .clone();
        let err = run(params).unwrap_err();
        assert!(
            matches!(err, ExecError::Node { ref message, .. } if message.contains("paths[1]")),
            "expected message pointing at `paths[1]`, got: {err:?}"
        );
    }

    // ---------- recursive param ----------

    #[test]
    fn non_recursive_errors_on_directory_default() {
        let dir = unique_tmp_dir("delete_dir_default");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("child.txt"), "x").unwrap();

        let path = dir.to_string_lossy().to_string();
        let params = json!({ "paths": path }).as_object().unwrap().clone();
        let err = run(params).unwrap_err();
        assert!(
            matches!(err, ExecError::Node { .. }),
            "non-recursive deleting a directory should error"
        );
        assert!(dir.exists(), "directory must survive a non-recursive attempt");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn recursive_deletes_directory_tree() {
        let dir = unique_tmp_dir("delete_dir_recursive");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.txt"), "x").unwrap();
        fs::create_dir_all(dir.join("nested")).unwrap();
        fs::write(dir.join("nested/b.txt"), "y").unwrap();

        let path = dir.to_string_lossy().to_string();
        let params = json!({ "paths": path, "recursive": true })
            .as_object()
            .unwrap()
            .clone();
        let out = run(params).unwrap();
        assert_eq!(out.as_str(), Some(path.as_str()));
        assert!(!dir.exists(), "directory tree should have been wiped");
    }

    #[test]
    fn recursive_still_deletes_a_single_file() {
        let file = TmpFile::new("delete_recursive_file");
        fs::write(&file.0, "z").unwrap();
        let path = file.0.to_string_lossy().to_string();
        let params = json!({ "paths": path, "recursive": true })
            .as_object()
            .unwrap()
            .clone();
        run(params).unwrap();
        assert!(!file.0.exists());
    }

    // ---------- port overrides param (future-proofing, mirrors file.pathPicker) ----------

    #[test]
    fn connected_paths_port_overrides_param_for_scalar() {
        let file = TmpFile::new("delete_port_override");
        fs::write(&file.0, "x").unwrap();
        let from_port_path = file.0.to_string_lossy().to_string();
        let from_port = Value::String(from_port_path.clone());

        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let mut inputs = HashMap::new();
        // `.to_string()` so the key is `String` (not `&str`) — matches
        // the `NodeInputs<'a> = HashMap<String, &'a Value>` signature.
        inputs.insert("paths".to_string(), &from_port);
        let params = json!({ "paths": "/never/exists/should/not/be/touched" })
            .as_object()
            .unwrap()
            .clone();
        let mut ctx = ctx_with("n1", "file.delete", &external, &mut outs, &mut logs);
        let r = exec(&inputs, &params, &mut ctx).unwrap();
        assert_eq!(
            r.get("deleted").unwrap().as_str(),
            Some(from_port_path.as_str())
        );
        assert!(!file.0.exists());
    }

    #[test]
    fn connected_paths_port_overrides_param_for_array() {
        // Worth pinning once the executor handles both shapes —
        // the future catalog might declare an array-shaped `paths`
        // input; this test would catch a regression where the port
        // path is dropped in favor of the textarea.
        let a = TmpFile::new("delete_port_array_a");
        let b = TmpFile::new("delete_port_array_b");
        fs::write(&a.0, "x").unwrap();
        fs::write(&b.0, "y").unwrap();
        let pa = a.0.to_string_lossy().to_string();
        let pb = b.0.to_string_lossy().to_string();
        let from_port = Value::Array(vec![
            Value::String(pa.clone()),
            Value::String(pb.clone()),
        ]);

        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let mut inputs = HashMap::new();
        inputs.insert("paths".to_string(), &from_port);
        let params = json!({ "paths": ["/never/a", "/never/b"] })
            .as_object()
            .unwrap()
            .clone();
        let mut ctx = ctx_with("n1", "file.delete", &external, &mut outs, &mut logs);
        let r = exec(&inputs, &params, &mut ctx).unwrap();
        let arr = r.get("deleted").unwrap().as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0].as_str(), Some(pa.as_str()));
        assert_eq!(arr[1].as_str(), Some(pb.as_str()));
        assert!(!a.0.exists());
        assert!(!b.0.exists());
    }
}
