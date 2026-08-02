//! Execution engine core types and helpers.
//!
//! Sub-modules:
//! - `broadcast` — numeric / pair broadcasting helpers over JSON values.
//! - `runner`    — `run_graph()` (Kahn topological sort + per-node execute).
//! - `nodes`     — one-file-per-node executors, dispatched by `kind`.
//!
//! Architecture contract:
//!   * `Value` is `serde_json::Value` (round-trips with the on-disk schema).
//!   * Each blueprint node kind is a small function (`NodeExec`) that takes
//!     a map of input port values + its own params and returns a map of
//!     output port values (`HashMap<port_id, Value>`). The hashmap shape
//!     means a node can declare any number of output ports (a future
//!     `file.read` may emit three: full text, lines array, raw file).
//!   * The dispatch table is built by `nodes::setup_registry()`.

use std::collections::HashMap;

use serde::Serialize;
use serde_json::{Map, Value};
use thiserror::Error;

pub mod broadcast;
pub mod nodes;
pub mod runner;

// Re-export so callers can `use executor::run_graph` without knowing the
// internal sub-module layout. Kept thin so `executor::runner::*` remains
// the canonical home for the orchestration primitives (helpers like
// `build_incoming`, `topo_sort`, the integration tests).
pub use runner::run_graph;

/// Per-call evaluation context. One per node execution.
pub struct EvalCtx<'a> {
    pub node_id: &'a str,
    pub kind: &'a str,
    pub external_inputs: &'a HashMap<String, Value>,
    pub outputs: &'a mut HashMap<String, Value>,
    pub logs: &'a mut Vec<LogEntry>,
}

/// A log entry written by an `io.logger` node.
#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    pub node_id: String,
    pub label: String,
    pub value: Value,
}

/// Inputs into a `NodeExec`: connected ports only.
pub type NodeInputs<'a> = HashMap<String, &'a Value>;
/// Outputs from a `NodeExec`: keyed by *output port id* (matches
/// `BlueprintNode.outputs[i].id`).
pub type NodeOutputs = HashMap<String, Value>;

/// Single node execution function. Cheap to register, easy to test.
pub type NodeExec = fn(
    inputs: &NodeInputs<'_>,
    params: &Map<String, Value>,
    ctx: &mut EvalCtx<'_>,
) -> Result<NodeOutputs, ExecError>;

#[derive(Debug, Error)]
pub enum ExecError {
    #[error("Cycle detected at node '{0}'")]
    Cycle(String),
    #[error("Unknown node kind '{0}'")]
    UnknownKind(String),
    #[error("Missing input '{port}' on node '{node}'")]
    MissingInput { node: String, port: String },
    #[error("Type mismatch on node '{node}': expected {expected}, got {actual}")]
    TypeMismatch {
        node: String,
        expected: String,
        actual: String,
    },
    #[error("Node '{node}': {message}")]
    Node { node: String, message: String },
    #[error("io.input '{name}' was not provided")]
    MissingInputValue { name: String },
}

// Serialize errors as a plain string so they cross the Tauri bridge.
impl Serialize for ExecError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RunResult {
    pub outputs: HashMap<String, Value>,
    pub logs: Vec<LogEntry>,
    pub stats: RunStats,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunStats {
    pub nodes_executed: usize,
    pub duration_ms: u64,
}

/// Stable reference to a shared `Value::Null` used as a default for
/// disconnected ports. Avoids the `unwrap_or(&Value::Null)` temporary
/// lifetime trap.
#[allow(dead_code)]
pub fn null() -> &'static Value {
    static N: Value = Value::Null;
    &N
}

/// Compact name of a JSON value's type, for error messages.
pub fn value_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Build `NodeOutputs` with a single port. Helper for executors that have
/// exactly one declared output.
pub fn single(port: impl Into<String>, value: Value) -> NodeOutputs {
    let mut m = HashMap::new();
    m.insert(port.into(), value);
    m
}

/// Empty outputs — for nodes whose only contribution is a side-effect
/// (`io.output`, `io.logger`).
pub fn empty_outputs() -> NodeOutputs {
    HashMap::new()
}

/// Weak value-to-string coercion used by string ops so that
/// `"prefix" + 5` doesn't error on type. Arrays/objects fall back to
/// their JSON representation.
pub fn value_to_string(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Convert a `f64` to a JSON `Value::Number`. NaN and ±∞ fall back to
/// Null so JSON stays well-formed.
pub fn f64_to_value(x: f64) -> Value {
    serde_json::Number::from_f64(x)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

// ---------------------------------------------------------------------
// Binary / single-or-array shape helpers.
//
// The binary pipeline introduces a `binary` port type whose payload
// travels as base64 inside `Value::String`. Several binary-related
// nodes (`file.readBinary`, `image.format`, `file.saveBinary`) all need
// to accept either a single value or an array of values and to align
// them with a sibling input (e.g. `files` vs `names`). These helpers
// factor that shape logic out so each executor stays compact.
// ---------------------------------------------------------------------

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;

/// Cardinality of a value flowing through a port — collapsed to three
/// cases the binary pipeline understands. `Empty` covers `Null`,
/// `Value::Array([])`, and `Value::String("")` because all three should
/// be treated as "not provided".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputShape {
    /// Single scalar (`Value::String` non-empty, or `Value::Number`/`Bool`).
    Single,
    /// Non-empty array of any length.
    Array(usize),
    /// Null / empty / wrong-type — treat as missing.
    Empty,
}

impl InputShape {
    pub fn is_empty(self) -> bool {
        matches!(self, InputShape::Empty)
    }
}

/// Classify a `Value` so callers can branch on scalar vs array without
/// re-matching `Value::String | Value::Array(...)` everywhere. Used by
/// every binary-pipeline executor; centralising the rule keeps
/// broadcast vs strict-mode semantics consistent across the trio.
pub fn classify_input(v: &Value) -> InputShape {
    match v {
        Value::String(s) if !s.is_empty() => InputShape::Single,
        Value::Array(arr) if !arr.is_empty() => InputShape::Array(arr.len()),
        _ => InputShape::Empty,
    }
}

/// Decode a base64 string into raw bytes. Returns `Err` with a friendly
/// message on failure — callers should wrap as `ExecError::Node`
/// naming the node they sit in.
///
/// **Convention:** every `Value::String` flowing through a `binary`
/// port is a base64-encoded payload. There is no length-prefix or
/// magic-byte header; if the source is suspicious the executor rejects
/// the value with `ExecError::Node` here so the executor body stays
/// straight-line.
pub fn decode_b64(input: &str) -> Result<Vec<u8>, String> {
    B64.decode(input).map_err(|e| format!("invalid base64: {e}"))
}

/// Encode raw bytes as base64 (no chunking). Inverse of `decode_b64`.
pub fn encode_b64(bytes: &[u8]) -> String {
    B64.encode(bytes)
}

/// Resolve how a (files, names) pair should align when one side is a
/// scalar and the other an array. `file.saveBinary` is the canonical
/// user; the rule is intentionally lenient for save-then-spread
/// (same file to N paths) but strict for save-then-overwrite-prone
/// (N files into one path probably means a wiring bug).
///
/// Returns either:
/// * `Ok(aligned)` — vector of `(file_value, name_value)` pairs ready
///   to iterate; lengths are guaranteed equal,
/// * `Err(ExecError::Node)` — caller-friendly error message.
///
/// **Rules:**
///   | files        | names        | outcome                  |
///   |--------------|--------------|--------------------------|
///   | Single       | Single       | one pair                 |
///   | Single       | Array(N)     | one file broadcast to N  |
///   | Array(N)     | Array(N)     | N pairs                  |
///   | Array(N)     | Single       | **error** — would overwrite |
///   | anything else | anything   | **error** — missing inputs |
pub fn align_binary_with_paths<'a>(
    files: &'a Value,
    names: &'a Value,
    node: &str,
) -> Result<Vec<(&'a Value, &'a Value)>, ExecError> {
    let f = classify_input(files);
    let n = classify_input(names);
    match (f, n) {
        (InputShape::Empty, _) | (_, InputShape::Empty) => Err(ExecError::Node {
            node: node.into(),
            message: "both `files` and `names` must be connected".into(),
        }),
        (InputShape::Single, InputShape::Single) => Ok(vec![(files, names)]),
        (InputShape::Single, InputShape::Array(n_size)) => {
            let mut out = Vec::with_capacity(n_size);
            for nm in names.as_array().unwrap() {
                out.push((files, nm));
            }
            Ok(out)
        }
        (InputShape::Array(f_size), InputShape::Array(n_size)) if f_size == n_size => {
            let mut out = Vec::with_capacity(f_size);
            for (f, nm) in files.as_array().unwrap().iter().zip(names.as_array().unwrap().iter()) {
                out.push((f, nm));
            }
            Ok(out)
        }
        (InputShape::Array(f_size), InputShape::Array(n_size)) => Err(ExecError::Node {
            node: node.into(),
            message: format!("files array ({f_size}) and names array ({n_size}) must have the same length"),
        }),
        (InputShape::Array(_), InputShape::Single) => Err(ExecError::Node {
            node: node.into(),
            message: "files array cannot be saved to a single name (would overwrite); connect an array of names".into(),
        }),
    }
}
