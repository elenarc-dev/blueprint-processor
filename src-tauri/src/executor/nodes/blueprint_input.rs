//! `blueprint.input` — Blender-style group-input node.
//!
//! Every `blueprint.input` instance in the graph produces the SAME
//! output set, derived from `BlueprintMetadata.inputs` on the parent
//! blueprint. The canonical list of inputs lives on the blueprint; the
//! node itself is just a connection-point into the shared bus.
//!
//! At run-time:
//!   * The runner pre-fills any missing key with the metadata-provided
//!     `default` (see `runner::run_graph`). By the time this executor
//!     runs, `external_inputs` is the keyed map of all declared inputs
//!     resolved.
//!   * This executor copies every entry into `NodeOutputs`, keyed by
//!     the metadata input id. Ports not present in `external_inputs`
//!     don't appear as outputs; the runner's per-port default of
//!     `Value::Null` then kicks in for any wired target (which is the
//!     right behaviour — a missing input is indistinguishable from a
//!     disconnected edge).
//!
//! No `params` are read. The output set is owned end-to-end by
//! `bp.metadata.inputs`; the catalog declares `outputs: []` to make
//! this contract explicit (the catalog is informational, the source
//! of truth is metadata).

use serde_json::{Map, Value};

use crate::executor::{EvalCtx, ExecError, NodeInputs, NodeOutputs};

pub fn exec(
    _inputs: &NodeInputs<'_>,
    _params: &Map<String, Value>,
    ctx: &mut EvalCtx<'_>,
) -> Result<NodeOutputs, ExecError> {
    // Iterate the (already default-filled) external map and emit one
    // output per key. We clone so the owned `NodeOutputs` outlives the
    // borrowed `ctx.external_inputs` reference.
    let mut outs: NodeOutputs = NodeOutputs::with_capacity(ctx.external_inputs.len());
    for (id, value) in ctx.external_inputs.iter() {
        outs.insert(id.clone(), value.clone());
    }
    Ok(outs)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::{json, Value};

    use super::exec;
    use crate::executor::nodes::test_helpers::{ctx_with, empty_inputs};

    #[test]
    fn emits_one_output_per_external_input_key() {
        let mut external = HashMap::new();
        external.insert("alpha".to_string(), Value::from(1));
        external.insert("beta".to_string(), Value::from("hello"));
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let null = Value::Null;
        let inputs = empty_inputs(&null);
        let mut ctx = ctx_with("n1", "blueprint.input", &external, &mut outs, &mut logs);
        let r = exec(
            &inputs,
            &json!({}).as_object().unwrap().clone(),
            &mut ctx,
        )
        .unwrap();
        // Only the configured keys; no marker output.
        assert_eq!(r.len(), 2);
        assert_eq!(r.get("alpha").unwrap().as_i64(), Some(1));
        assert_eq!(r.get("beta").unwrap().as_str(), Some("hello"));
    }

    #[test]
    fn empty_external_inputs_yields_zero_outputs() {
        let external: HashMap<String, Value> = HashMap::new();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let null = Value::Null;
        let inputs = empty_inputs(&null);
        let mut ctx = ctx_with("n1", "blueprint.input", &external, &mut outs, &mut logs);
        let r = exec(
            &inputs,
            &json!({}).as_object().unwrap().clone(),
            &mut ctx,
        )
        .unwrap();
        assert!(r.is_empty());
    }

    #[test]
    fn copies_values_verbatim_including_object_array() {
        let mut external = HashMap::new();
        external.insert("arr".to_string(), json!([1, 2, 3]));
        external.insert("obj".to_string(), json!({"k": "v"}));
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let null = Value::Null;
        let inputs = empty_inputs(&null);
        let mut ctx = ctx_with("n1", "blueprint.input", &external, &mut outs, &mut logs);
        let r = exec(
            &inputs,
            &json!({}).as_object().unwrap().clone(),
            &mut ctx,
        )
        .unwrap();
        assert_eq!(r.get("arr").unwrap(), &json!([1, 2, 3]));
        assert_eq!(r.get("obj").unwrap(), &json!({"k": "v"}));
    }
}
