//! Per-node executors. One module file per `kind`. `setup_registry()`
//! builds the dispatch table consumed by `runner::run_graph`.
//!
//! Conventions for every node module file:
//! - public `pub fn exec(...) -> Result<NodeOutputs, ExecError>`
//! - `#[cfg(test)] mod tests { ... }` with the node's own unit tests.

use std::collections::HashMap;

use crate::executor::NodeExec;

pub mod array_create;
pub mod array_filter;
pub mod array_get_field;
pub mod array_length;
pub mod array_push;
pub mod array_set_field;
pub mod array_sum;
pub mod blueprint_input;
pub mod file_delete;
pub mod file_iterate_folder;
pub mod file_path_picker;
pub mod file_read;
pub mod file_save_binary;
pub mod file_write;
pub mod image_format;
pub mod io_input;
pub mod io_logger;
pub mod io_output;
pub mod json_parse;
pub mod json_read_file;
pub mod json_stringify;
pub mod literal_boolean;
pub mod literal_json;
pub mod literal_number;
pub mod literal_string;
pub mod math_add;
pub mod math_compare;
pub mod math_divide;
pub mod math_multiply;
pub mod math_subtract;
pub mod string_concat;
pub mod string_filter;
pub mod string_join;
pub mod string_length;
pub mod string_replace;
pub mod string_split;

/// Build the dispatch table mapping a blueprint `kind` to its executor fn.
pub fn setup_registry() -> HashMap<String, NodeExec> {
    let mut r: HashMap<String, NodeExec> = HashMap::new();

    // Literals
    r.insert("literal.number".into(), literal_number::exec as NodeExec);
    r.insert("literal.string".into(), literal_string::exec as NodeExec);
    r.insert("literal.boolean".into(), literal_boolean::exec as NodeExec);
    r.insert("literal.json".into(), literal_json::exec as NodeExec);

    // Math
    r.insert("math.add".into(), math_add::exec as NodeExec);
    r.insert("math.subtract".into(), math_subtract::exec as NodeExec);
    r.insert("math.multiply".into(), math_multiply::exec as NodeExec);
    r.insert("math.divide".into(), math_divide::exec as NodeExec);
    r.insert("math.compare".into(), math_compare::exec as NodeExec);

    // String
    r.insert("string.concat".into(), string_concat::exec as NodeExec);
    r.insert("string.length".into(), string_length::exec as NodeExec);
    r.insert("string.split".into(), string_split::exec as NodeExec);
    r.insert("string.join".into(), string_join::exec as NodeExec);
    r.insert("string.replace".into(), string_replace::exec as NodeExec);
    r.insert("array.textfilter".into(), string_filter::exec as NodeExec);

    // Array
    r.insert("array.length".into(), array_length::exec as NodeExec);
    r.insert("array.filter".into(), array_filter::exec as NodeExec);
    r.insert("array.sum".into(), array_sum::exec as NodeExec);
    r.insert("array.push".into(), array_push::exec as NodeExec);
    r.insert("array.create".into(), array_create::exec as NodeExec);
    r.insert("array.getField".into(), array_get_field::exec as NodeExec);
    r.insert("array.setField".into(), array_set_field::exec as NodeExec);

    // IO. Note: `file.read` now emits `text`, `lines`, AND `binary`
    // ports, so the standalone `file.readBinary` kind that used to
    // live here is gone — duplicating the read pattern was confusing
    // and the catalog duplicate is removed in `lib/nodes/catalog.ts`.
    r.insert("file.iterate".into(), file_iterate_folder::exec as NodeExec);
    r.insert("file.pathPicker".into(), file_path_picker::exec as NodeExec);
    r.insert("io.input".into(), io_input::exec as NodeExec);
    r.insert("io.output".into(), io_output::exec as NodeExec);
    r.insert("io.logger".into(), io_logger::exec as NodeExec);
    r.insert("file.read".into(), file_read::exec as NodeExec);
    r.insert("file.saveBinary".into(), file_save_binary::exec as NodeExec);
    r.insert("file.write".into(), file_write::exec as NodeExec);
    r.insert("file.delete".into(), file_delete::exec as NodeExec);

    // Image
    r.insert("image.format".into(), image_format::exec as NodeExec);

    // Blueprint-level (`bp.metadata.inputs`) shared input bus.
    // Registered next to legacy `io.input` so blueprints authored
    // against the old kind still load and execute.
    r.insert("blueprint.input".into(), blueprint_input::exec as NodeExec);

    // JSON
    r.insert("json.parse".into(), json_parse::exec as NodeExec);
    r.insert("json.stringify".into(), json_stringify::exec as NodeExec);
    r.insert("json.readFile".into(), json_read_file::exec as NodeExec);

    r
}

// ---------------------------------------------------------------------
// Test helpers — shared by every per-node test module so we don't
// duplicate boilerplate 19 times.
// ---------------------------------------------------------------------

#[cfg(test)]
pub mod test_helpers {
    use std::collections::HashMap;

    use serde_json::Value;

    use crate::executor::{EvalCtx, LogEntry, NodeInputs};

    /// Build a default `EvalCtx` with empty output + log buffers.
    pub fn ctx_with<'a>(
        node_id: &'a str,
        kind: &'a str,
        external: &'a HashMap<String, Value>,
        outputs: &'a mut HashMap<String, Value>,
        logs: &'a mut Vec<LogEntry>,
    ) -> EvalCtx<'a> {
        EvalCtx {
            node_id,
            kind,
            external_inputs: external,
            outputs,
            logs,
        }
    }

    /// Build a `NodeInputs` map with a single `__null` entry. Useful for
    /// tests of executors that don't read any specific port.
    pub fn empty_inputs<'a>(null: &'a Value) -> NodeInputs<'a> {
        let mut m = HashMap::new();
        m.insert("__null".to_string(), null);
        m
    }

    /// Build a fresh path inside the system temp directory, prefixed by
    /// `label` and disambiguated by `pid + nanos`. Used by file-touching
    /// tests so parallel `cargo test` runs do not collide on the same
    /// path across threads (or across CI shards).
    pub fn unique_tmp(label: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let pid = std::process::id();
        let mut p = std::env::temp_dir();
        p.push(format!("bp-test-{label}-{pid}-{nanos}.txt"));
        p
    }

    /// RAII guard for temp test files. `Drop` removes the file even if
    /// a test panics between `write` and `remove_file`; the OS tempdir
    /// stays clean across runs. Pair with `fs::write` to seed the
    /// file, then access `.0` (or `as_path()`) for the path string.
    pub struct TmpFile(pub std::path::PathBuf);

    impl TmpFile {
        /// Allocate a fresh guarded path under the system temp dir.
        pub fn new(label: &str) -> Self {
            TmpFile(unique_tmp(label))
        }
    }

    impl Drop for TmpFile {
        fn drop(&mut self) {
            // Best-effort; don't panic in cleanup. Directories created
            // by nested-path tests (file.write's recursive parent case)
            // are still cleaned manually because TmpFile only unlinks
            // files — that's a deliberate, conscious choice.
            let _ = std::fs::remove_file(&self.0);
        }
    }
}
