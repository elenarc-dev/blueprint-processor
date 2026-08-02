//! Blueprint data definitions.
//!
//! The on-disk `.blueprint.json` format. Designed to be forward-compatible
//! so new node kinds can be added later without breaking existing files.

use serde::{Deserialize, Serialize};

/// Current on-disk schema version. Bumped when an incompatible change is made.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Blueprint {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    pub nodes: Vec<BlueprintNode>,
    pub connections: Vec<BlueprintConnection>,
    #[serde(default)]
    pub metadata: BlueprintMetadata,
}

/// A single user-declared input on a blueprint. The canonical list lives
/// on `BlueprintMetadata.inputs` and is shared across every
/// `blueprint.input` node in the graph — Blender group-input
/// semantics, where the group interface is one thing and every group
/// node in the graph surfaces it.
///
/// The `data_type` field is informational and used for input-modal
/// rendering on the frontend. At run-time the executor reads `external_inputs`
/// keyed by `id` regardless of declared type, so a wrong type on disk
/// is a user-misconfiguration, not a crash.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlueprintInput {
    /// Stable, unique id used as the runtime key. Must match `regex ^[a-zA-Z0-9_]+$`
    /// (enforced on the Inspector side; the executor is permissive).
    pub id: String,
    /// Human label for the dashboard modal and palette tooltip.
    pub name: String,
    /// `"string" | "number" | "boolean" | "path" | "array" | "any"`.
    #[serde(rename = "type")]
    pub data_type: String,
    /// Optional default used when no runtime value is supplied. The
    /// runner reads this and pre-fills `external_inputs` before the
    /// topological loop so `blueprint.input::exec` sees a value.
    #[serde(default)]
    pub default: Option<serde_json::Value>,
    /// Free-text description shown next to the field in the dashboard
    /// modal.
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BlueprintMetadata {
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Blender-style inputs. Every `blueprint.input` node in the graph
    /// surfaces this set as its output ports.
    #[serde(default)]
    pub inputs: Vec<BlueprintInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlueprintNode {
    pub id: String,
    /// Stable kind identifier (e.g. `"math.add"`, `"string.concat"`).
    pub kind: String,
    #[serde(default)]
    pub category: Option<String>,
    /// Visual position inside the editor canvas.
    pub position: NodePosition,
    /// Human-readable label shown on the node header.
    #[serde(default)]
    pub label: Option<String>,
    /// Node-specific configuration values (literal value, math op, etc.).
    #[serde(default)]
    pub params: serde_json::Value,
    /// Optional list of input ports. Mostly derivable from `kind`, but
    /// included so custom ports (e.g. array length) can be persisted.
    #[serde(default)]
    pub inputs: Vec<Port>,
    /// Optional list of output ports.
    #[serde(default)]
    pub outputs: Vec<Port>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodePosition {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Port {
    pub id: String,
    pub name: String,
    /// `"number" | "string" | "boolean" | "array" | "any"`.
    #[serde(rename = "type")]
    pub data_type: String,
}

/// On-disk JSON uses camelCase keys (matches TypeScript BlueprintConnection
/// interface). Field names stay snake_case in Rust for convention.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlueprintConnection {
    pub id: String,
    pub source_node: String,
    pub source_port: String,
    pub target_node: String,
    pub target_port: String,
}

impl Blueprint {
    /// Build a Blueprint with sensible defaults. Used to scaffold empty
    /// blueprints when the user hits "New".
    pub fn empty(name: impl Into<String>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            id: String::new(),
            name: name.into(),
            description: String::new(),
            created_at: None,
            updated_at: None,
            nodes: Vec::new(),
            connections: Vec::new(),
            metadata: BlueprintMetadata::default(),
        }
    }
}
