//! Graph runner: Kahn's topological sort + per-node execution.
//!
//! The runner is the orchestration layer. Each node is executed exactly
//! once in topological order; its output values are threaded into
//! downstream nodes' input maps. `io.input` reads from the
//! `external_inputs` map; `io.output` writes into the result's outputs;
//! `io.logger` appends to the result's logs.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde_json::{Map, Value};

use crate::ai::{AiEnv, AiGateway, UnconfiguredGateway};
use crate::blueprint::{Blueprint, BlueprintConnection, BlueprintNode};

use super::nodes;
use super::{
    EvalCtx, ExecError, LogEntry, NodeExec, NodeInputs, RunFailure, RunResult, RunStats,
};

/// Run a blueprint graph and return the result.
///
/// **Default-fill pass:** before the topological loop, every key
/// declared on `bp.metadata.inputs` that is absent from
/// `external_inputs` is filled with the declared `default`. This way
/// `blueprint.input::exec` only ever sees a fully-resolved map and
/// missing inputs collapse to the right fallback (declared default →
/// `Value::Null` for any undeclared/missing combination). The original
/// `external_inputs` map is not mutated; we clone and shadow.
/// Run a graph without an AI gateway: every `ai.*` node fails with
/// `MissingKey`. Kept as the simple entry point for tests and for any
/// future caller that only runs pure nodes.
#[allow(dead_code)]
pub async fn run_graph(
    bp: &Blueprint,
    external_inputs: &HashMap<String, Value>,
) -> Result<RunResult, RunFailure> {
    run_graph_with(bp, external_inputs, Arc::new(UnconfiguredGateway)).await
}

/// [`run_graph`] with an explicit AI gateway.
///
/// The Tauri command supplies the real, secret-store-backed gateway; tests
/// inject `ai::mock::MockGateway` so `cargo test` never needs a key or a
/// network connection. A caller that passes no gateway (`run_graph`) gets
/// [`UnconfiguredGateway`], where any `ai.*` node fails loudly instead of
/// silently returning placeholder text.
pub async fn run_graph_with(
    bp: &Blueprint,
    external_inputs: &HashMap<String, Value>,
    gateway: Arc<dyn AiGateway>,
) -> Result<RunResult, RunFailure> {
    let started = std::time::Instant::now();
    let registry = nodes::setup_registry();
    let async_registry = nodes::setup_async_registry();
    let incoming = build_incoming(&bp.connections);
    let order = topo_sort(&bp.nodes, &bp.connections).map_err(RunFailure::from)?;
    let mut effective = external_inputs.clone();
    apply_metadata_defaults(&mut effective, &bp.metadata.inputs);
    execute(
        &order,
        &bp.nodes,
        &incoming,
        &registry,
        &async_registry,
        &effective,
        &gateway,
        started,
    )
    .await
}

/// For each declared `BlueprintInput` without a value in `effective`,
/// insert its `default` (if any). Done by the runner so the executor
/// never has to know about metadata semantics.
fn apply_metadata_defaults(
    effective: &mut HashMap<String, Value>,
    declared: &[crate::blueprint::BlueprintInput],
) {
    for input in declared {
        if effective.contains_key(&input.id) {
            continue;
        }
        if let Some(ref default) = input.default {
            effective.insert(input.id.clone(), default.clone());
        }
    }
}

/// Build a `target_node -> port_id -> (source_node, source_port)` map for
/// quick lookup during node execution.
fn build_incoming(
    connections: &[BlueprintConnection],
) -> HashMap<String, HashMap<String, (String, String)>> {
    let mut incoming: HashMap<String, HashMap<String, (String, String)>> = HashMap::new();
    for c in connections {
        incoming
            .entry(c.target_node.clone())
            .or_default()
            .insert(
                c.target_port.clone(),
                (c.source_node.clone(), c.source_port.clone()),
            );
    }
    incoming
}

/// Kahn's topological sort. Returns execution order, or `Cycle` if the
/// graph has a cycle.
///
/// Edge dedup keys on the FULL 4-tuple (src, sport, target, tport) so
/// two literal connections that happen to point at the same source/
/// target pair collapse into one. The `port_pending` map keys on
/// `(target, tport)` — the per-port outstanding-edge count. A node
/// becomes ready when every one of its input ports has 0 pending.
///
/// **Why per-port rather than per-node:** a single source feeding N
/// ports of the same target gets one decrement per port, not one
/// decrement per (source, target) pair. The previous implementation
/// used an `in_degree` per node and a `successors` HashSet; the
/// mismatch caused a false-positive `Cycle` whenever a source node
/// fed more than one port of the same target (real-world bug reported
/// by a user running a four-node DAG with two feeds of the same
/// `array.create` ports).
fn topo_sort(
    nodes: &[BlueprintNode],
    connections: &[BlueprintConnection],
) -> Result<Vec<String>, ExecError> {
    // 1) Edge dedup: collapse literal duplicates of the same wire.
    let mut seen: HashSet<(String, String, String, String)> = HashSet::new();
    let mut deduped: Vec<&BlueprintConnection> = Vec::new();
    for c in connections {
        let key = (
            c.source_node.clone(),
            c.source_port.clone(),
            c.target_node.clone(),
            c.target_port.clone(),
        );
        if seen.insert(key) {
            deduped.push(c);
        }
    }

    // 2) Per-port outstanding-edge count after dedup.
    let mut port_pending: HashMap<(String, String), usize> = HashMap::new();
    for c in &deduped {
        *port_pending
            .entry((c.target_node.clone(), c.target_port.clone()))
            .or_insert(0) += 1;
    }

    /// A node is ready iff every one of its declared input ports has
    /// no outstanding edges.
    fn node_ready(
        node: &BlueprintNode,
        port_pending: &HashMap<(String, String), usize>,
    ) -> bool {
        node.inputs.iter().all(|p| {
            port_pending
                .get(&(node.id.clone(), p.id.clone()))
                .copied()
                .unwrap_or(0)
                == 0
        })
    }

    // Logger-priority classification. When the runner's frontier
    // holds multiple READY siblings that share a common upstream
    // producer — i.e. they branch off the same, just-completed
    // node in the SAME execution round — ANY node whose kind
    // starts with `io.log` (today just `io.logger`; the prefix
    // leaves room for future `io.logMetric`-style siblings)
    // runs before the others. The user wants this so the run
    // log captures debug values co-temporally with whatever the
    // sibling nodes are about to do, even if a later sibling
    // errors out and the rest of the DAG is skipped.
    //
    // SCOPE NOTE: the rule is "siblings READY in the same round",
    // NOT "logger absolutely first under any topology". Two
    // nodes that branch off DIFFERENT upstream producers settle
    // on the topological order determined by Kahn. A node that
    // was already committed from an earlier alphabetised sub-
    // frontier may complete before a logger that becomes ready
    // in a later unblock — that's intentional and within the
    // sibling-scope rule (the earlier non-logger has no shared
    // upstream with the later logger).
    //
    // The classification is computed ONCE up front so neither
    // sort below does an O(n*m) look-up: a single
    // `HashSet::contains` per compare.
    let mut logger_priority: HashSet<String> = HashSet::new();
    for n in nodes {
        if n.kind.starts_with("io.log") {
            logger_priority.insert(n.id.clone());
        }
    }
    /// Comparator closure: loggers first, otherwise alphabetical.
    /// Reused at both `frontier.sort_by` and `pushed.sort_by` so a
    /// future tweak (e.g. lower the priority class for a different
    /// purpose) lands in lock-step across both sorts.
    let by_priority_then_id = |a: &String, b: &String| -> std::cmp::Ordering {
        let a_pri = if logger_priority.contains(a) { 0 } else { 1 };
        let b_pri = if logger_priority.contains(b) { 0 } else { 1 };
        a_pri.cmp(&b_pri).then_with(|| a.cmp(b))
    };

    // 3) Kahn iteration. Decrement per PORT (not per successor node)
    // so a source feeding multiple ports of one target resolves them
    // all at once.
    let mut frontier: Vec<String> = nodes
        .iter()
        .filter(|n| node_ready(n, &port_pending))
        .map(|n| n.id.clone())
        .collect();
    frontier.sort_by(|a, b| by_priority_then_id(a, b));

    let mut order: Vec<String> = Vec::with_capacity(nodes.len());
    let mut pushed: Vec<String> = Vec::new();
    // `executed` tracks nodes already added to `order`. A second visit
    // of an already-executed node is the unambiguous signature of a
    // cycle (a well-formed DAG never re-emits a node). Catching it here
    // also avoids the integer-underflow trap on `port_pending[k] -= 1`
    // when a back-edge would otherwise drive `d` below 0.
    let mut executed: HashSet<String> = HashSet::new();
    while let Some(id) = frontier.first().cloned() {
        frontier.remove(0);
        if !executed.insert(id.clone()) {
            return Err(ExecError::Cycle(id));
        }
        order.push(id.clone());

        for c in deduped.iter().filter(|c| c.source_node == id) {
            let key = (c.target_node.clone(), c.target_port.clone());
            if let Some(d) = port_pending.get_mut(&key) {
                *d -= 1;
                if *d == 0 {
                    let tgt_id = c.target_node.clone();
                    if let Some(tgt) = nodes.iter().find(|n| n.id == tgt_id) {
                        if node_ready(tgt, &port_pending) {
                            pushed.push(tgt_id);
                        }
                    }
                }
            }
        }
    pushed.sort_by(|a, b| by_priority_then_id(a, b));
    pushed.dedup();
    frontier.extend(pushed.drain(..));
    }

    if order.len() != nodes.len() {
        let culprit = nodes
            .iter()
            .find(|n| !node_ready(n, &port_pending))
            .map(|n| n.id.clone())
            .unwrap_or_else(|| "?".to_string());
        return Err(ExecError::Cycle(culprit));
    }
    Ok(order)
}

#[allow(clippy::too_many_arguments)]
async fn execute(
    order: &[String],
    bp_nodes: &[BlueprintNode],
    incoming: &HashMap<String, HashMap<String, (String, String)>>,
    registry: &HashMap<String, NodeExec>,
    async_registry: &HashMap<String, super::AsyncNodeExec>,
    external_inputs: &HashMap<String, Value>,
    gateway: &Arc<dyn AiGateway>,
    started: std::time::Instant,
) -> Result<RunResult, RunFailure> {
    let mut outputs_produced: HashMap<(String, String), Value> = HashMap::new();
    let mut final_outputs: HashMap<String, Value> = HashMap::new();
    let mut logs: Vec<LogEntry> = Vec::new();
    let mut executed: usize = 0;

    let null_value = Value::Null;

    for node_id in order {
        let bp_node = match bp_nodes.iter().find(|n| &n.id == node_id) {
            Some(n) => n,
            None => {
                return Err(fail(
                    ExecError::Node {
                        node: node_id.clone(),
                        message: "missing from blueprint during execution".into(),
                    },
                    logs,
                    executed,
                    started,
                ))
            }
        };

        // Dispatch: a kind lives in exactly one of the two tables. Async
        // is checked first because async executors need owned inputs,
        // which are collected below.
        let sync_executor = registry.get(&bp_node.kind).copied();
        let async_executor = async_registry.get(&bp_node.kind).copied();
        if sync_executor.is_none() && async_executor.is_none() {
            return Err(fail(
                ExecError::UnknownKind(bp_node.kind.clone()),
                logs,
                executed,
                started,
            ));
        }

        // Coerce the frontend-typed inline defaults (`params[<port_id>]`
        // keys) into a HashMap for executor-side lookup. The frontend
        // SHOULD always serialise as Object, but we tolerate a missing
        // or non-Object `params` by treating it as empty — a node that
        // never recorded any inline default simply hands Null to the
        // executor on unwired ports, exactly like the pre-feature
        // behaviour.
        let params = bp_node.params.as_object().cloned().unwrap_or_default();

        // Collect per-port inputs. Disconnected ports default to Null,
        // but the per-port `params[<port_id>]` becomes the inline
        // default whenever the wire is unwired OR the upstream
        // produced Null — Blender-style "input deflection" / "value
        // pin" UX (the user types a default directly on the canvas
        // and the wire still wins when connected and non-Null). This is
        // non-breaking: nodes that don't declare a `params[<port_id>]`
        // entry fall through to `&null_value` exactly as before.
        let port_map = incoming.get(node_id);
        let mut inputs: NodeInputs = HashMap::with_capacity(bp_node.inputs.len());
        for port_in in &bp_node.inputs {
            // Always look up the wire first; the wire still wins when
            // it's present and produces a non-Null value.
            let wired_ref: Option<&Value> = port_map
                .and_then(|m| m.get(&port_in.id))
                .and_then(|(src, sport)| {
                    outputs_produced.get(&(src.clone(), sport.clone()))
                });
            let value: &Value = match wired_ref {
                Some(v) if !matches!(v, Value::Null) => v,
                // Either the port is unwired OR the upstream slipped a
                // Null through (rare, but documented for ports like
                // string.replace where inputs.find might be Null during
                // the port-vs-param fallback chain). In either case the
                // user's per-port inline default wins. Note this
                // collapses the "user explicitly wired a Null" case
                // to "fall back to params" too, which matches the
                // principle: if there is no RUNTIME value, use the
                // configured default. Sending an explicit Null across
                // a wire is unusual and a downstream node will most
                // often benefit from this fallback anyway.
                _ => params.get(&port_in.id).unwrap_or(&null_value),
            };
            inputs.insert(port_in.id.clone(), value);
        }

        // Async branch (AI nodes). Inputs are cloned into owned values so
        // the boxed future holds no borrow of `outputs_produced`, then the
        // borrowed map is dropped before the await. Only `ai.*` kinds take
        // this path, so the clone cost lands on AI payloads alone.
        if let Some(run_async) = async_executor {
            let owned_inputs: Map<String, Value> = inputs
                .iter()
                .map(|(k, v)| (k.clone(), (*v).clone()))
                .collect();
            drop(inputs);
            let env = AiEnv {
                node_id: node_id.clone(),
                gateway: Arc::clone(gateway),
            };
            let outs = match run_async(&owned_inputs, &params, &env).await {
                Ok(outs) => outs,
                Err(e) => return Err(fail(e, logs, executed, started)),
            };
            executed += 1;
            for (port_id, val) in outs {
                outputs_produced.insert((node_id.clone(), port_id), val);
            }
            continue;
        }

        let executor = sync_executor
            .expect("validated above: the kind is in the sync or the async registry");
        // Scope the ctx so its `&mut logs` borrow ends before the error
        // path moves `logs` into the failure.
        let res = {
            let mut ctx = EvalCtx {
                node_id,
                kind: &bp_node.kind,
                external_inputs,
                outputs: &mut final_outputs,
                logs: &mut logs,
            };
            executor(&inputs, &params, &mut ctx)
        };
        let outs = match res {
            Ok(outs) => outs,
            Err(e) => return Err(fail(e, logs, executed, started)),
        };
        executed += 1;
        for (port_id, val) in outs {
            outputs_produced.insert((node_id.clone(), port_id), val);
        }
    }

    Ok(RunResult {
        outputs: final_outputs,
        logs,
        stats: RunStats {
            nodes_executed: executed,
            duration_ms: started.elapsed().as_millis() as u64,
        },
    })
}

/// Wrap a node error with the logs collected so far. Called on every
/// `return` that aborts a run, so no diagnostic work is ever discarded.
fn fail(
    error: ExecError,
    logs: Vec<LogEntry>,
    executed: usize,
    started: std::time::Instant,
) -> RunFailure {
    RunFailure {
        error,
        logs,
        stats: RunStats {
            nodes_executed: executed,
            duration_ms: started.elapsed().as_millis() as u64,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blueprint::{Blueprint, BlueprintConnection, BlueprintNode, NodePosition, Port};
    use serde_json::json;

    fn port(id: &str, name: &str, data_type: &str) -> Port {
        Port { id: id.into(), name: name.into(), data_type: data_type.into() }
    }

    fn make_blueprint(
        nodes: Vec<BlueprintNode>,
        connections: Vec<BlueprintConnection>,
    ) -> Blueprint {
        Blueprint {
            schema_version: 1,
            id: "test".into(),
            name: "test".into(),
            description: String::new(),
            created_at: None,
            updated_at: None,
            nodes,
            connections,
            metadata: Default::default(),
        }
    }

    /// `run_graph` is async because AI nodes await provider calls; the
    /// existing tests stay synchronous through Tauri's shared runtime
    /// (no extra runtime dependency in the test harness).
    fn run_sync(
        bp: &Blueprint,
        ext: &HashMap<String, Value>,
    ) -> Result<RunResult, RunFailure> {
        tauri::async_runtime::block_on(run_graph(bp, ext))
    }

    #[test]
    fn ai_node_output_feeds_downstream_split() {
        // AI-TEST-006. The task's example was `[CSV] -> [AI] -> [String
        // Split]`; there is no CSV node in the catalog, so the CSV row is
        // produced by `literal.string` — same graph shape, real nodes.
        // The model call goes through `MockGateway`, so the test is
        // offline and deterministic.
        use crate::ai::mock::MockGateway;
        use crate::ai::AiGateway;

        fn commas_to_semicolons(prompt: &str) -> String {
            prompt.replace(',', ";")
        }

        let bp = make_blueprint(
            vec![
                BlueprintNode {
                    id: "csv".into(),
                    kind: "literal.string".into(),
                    category: None,
                    position: NodePosition { x: 0.0, y: 0.0 },
                    label: None,
                    params: json!({"value": "1,2,3"}),
                    inputs: vec![],
                    outputs: vec![port("value", "Value", "string")],
                },
                BlueprintNode {
                    id: "ai".into(),
                    kind: "ai.completion".into(),
                    category: None,
                    position: NodePosition { x: 0.0, y: 0.0 },
                    label: None,
                    // No provider key here on purpose: credentials come
                    // from the gateway, never from the blueprint.
                    params: json!({"provider": "openai", "model": "test-model"}),
                    inputs: vec![
                        port("then_in", "Then", "exec"),
                        port("prompt", "Prompt", "string-or-array"),
                    ],
                    outputs: vec![
                        port("then_out", "Then", "exec"),
                        port("out", "Out", "string-or-array"),
                    ],
                },
                BlueprintNode {
                    id: "split".into(),
                    kind: "string.split".into(),
                    category: None,
                    position: NodePosition { x: 0.0, y: 0.0 },
                    label: None,
                    params: json!({"sep": ";"}),
                    inputs: vec![
                        port("then_in", "Then", "exec"),
                        port("in", "In", "string"),
                    ],
                    outputs: vec![
                        port("then_out", "Then", "exec"),
                        port("out", "Out", "array"),
                    ],
                },
                BlueprintNode {
                    id: "count".into(),
                    kind: "array.length".into(),
                    category: None,
                    position: NodePosition { x: 0.0, y: 0.0 },
                    label: None,
                    params: json!({}),
                    inputs: vec![
                        port("then_in", "Then", "exec"),
                        port("in", "In", "array"),
                    ],
                    outputs: vec![
                        port("then_out", "Then", "exec"),
                        port("out", "Length", "number"),
                    ],
                },
                BlueprintNode {
                    id: "out".into(),
                    kind: "io.output".into(),
                    category: None,
                    position: NodePosition { x: 0.0, y: 0.0 },
                    label: None,
                    params: json!({"name": "count"}),
                    inputs: vec![
                        port("then_in", "Then", "exec"),
                        port("in", "In", "any"),
                    ],
                    outputs: vec![],
                },
            ],
            vec![
                BlueprintConnection {
                    id: "e1".into(),
                    source_node: "csv".into(),
                    source_port: "value".into(),
                    target_node: "ai".into(),
                    target_port: "prompt".into(),
                },
                BlueprintConnection {
                    id: "e2".into(),
                    source_node: "ai".into(),
                    source_port: "out".into(),
                    target_node: "split".into(),
                    target_port: "in".into(),
                },
                BlueprintConnection {
                    id: "e3".into(),
                    source_node: "split".into(),
                    source_port: "out".into(),
                    target_node: "count".into(),
                    target_port: "in".into(),
                },
                BlueprintConnection {
                    id: "e4".into(),
                    source_node: "count".into(),
                    source_port: "out".into(),
                    target_node: "out".into(),
                    target_port: "in".into(),
                },
            ],
        );

        let mock = Arc::new(MockGateway::new(commas_to_semicolons));
        let gateway: Arc<dyn AiGateway> = mock.clone();
        let res = tauri::async_runtime::block_on(run_graph_with(
            &bp,
            &HashMap::new(),
            gateway,
        ))
        .expect("AI pipeline must run");

        assert_eq!(res.stats.nodes_executed, 5);
        // The model rewrote ',' to ';', string.split then produced three
        // values and array.length counted them: the AI output really
        // flowed into another functional node.
        assert_eq!(
            res.outputs.get("count").and_then(|v| v.as_f64()),
            Some(3.0)
        );
        let calls = mock.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].prompt, "1,2,3");
        assert_eq!(calls[0].provider.as_str(), "openai");
    }

    #[test]
    fn full_run_graph_literal_add_output() {
        let bp = make_blueprint(
            vec![
                BlueprintNode {
                    id: "lit".into(),
                    kind: "literal.number".into(),
                    category: None,
                    position: NodePosition { x: 0.0, y: 0.0 },
                    label: None,
                    params: json!({"value": 5}),
                    inputs: vec![],
                    outputs: vec![port("value", "Value", "number")],
                },
                BlueprintNode {
                    id: "add".into(),
                    kind: "math.add".into(),
                    category: None,
                    position: NodePosition { x: 0.0, y: 0.0 },
                    label: None,
                    params: json!({}),
                    inputs: vec![port("a", "A", "number"), port("b", "B", "number")],
                    outputs: vec![port("sum", "Sum", "number")],
                },
                BlueprintNode {
                    id: "lit2".into(),
                    kind: "literal.number".into(),
                    category: None,
                    position: NodePosition { x: 0.0, y: 0.0 },
                    label: None,
                    params: json!({"value": 3}),
                    inputs: vec![],
                    outputs: vec![port("value", "Value", "number")],
                },
                BlueprintNode {
                    id: "out".into(),
                    kind: "io.output".into(),
                    category: None,
                    position: NodePosition { x: 0.0, y: 0.0 },
                    label: None,
                    params: json!({"name": "result"}),
                    inputs: vec![port("in", "In", "any")],
                    outputs: vec![],
                },
            ],
            vec![
                BlueprintConnection {
                    id: "e1".into(),
                    source_node: "lit".into(),
                    source_port: "value".into(),
                    target_node: "add".into(),
                    target_port: "a".into(),
                },
                BlueprintConnection {
                    id: "e2".into(),
                    source_node: "lit2".into(),
                    source_port: "value".into(),
                    target_node: "add".into(),
                    target_port: "b".into(),
                },
                BlueprintConnection {
                    id: "e3".into(),
                    source_node: "add".into(),
                    source_port: "sum".into(),
                    target_node: "out".into(),
                    target_port: "in".into(),
                },
            ],
        );
        let ext = HashMap::new();
        let res = run_sync(&bp, &ext).unwrap();
        // math.add serialises through `f64_to_value` → Number::Float;
        // serde_json's `as_i64` only returns Some for integer-stored
        // numbers, so compare via `as_f64`.
        assert_eq!(
            res.outputs.get("result").and_then(|v| v.as_f64()),
            Some(8.0)
        );
        assert_eq!(res.stats.nodes_executed, 4);
        assert_eq!(res.logs.len(), 0);
    }

    #[test]
    fn duplicate_literal_edges_do_not_trip_cycle_check() {
        // Regression for a real-world bug: Svelte Flow / save_blueprint
        // could emit the same (src, target, port) pair twice with
        // different edge ids. The previous `in_degree` was keyed on
        // `(target, port)` but Kahn decremented per UNIQUE successor
        // — so a source feeding N ports of one target decremented once
        // while it contributed N to `in_degree`. Result: a perfectly
        // valid DAG tripped a false `Cycle`. The fix dedups by full
        // (src, sport, tgt, tport) tuple and tracks per-port pending.
        let bp = make_blueprint(
            vec![
                BlueprintNode {
                    id: "n_lit".into(),
                    kind: "literal.number".into(),
                    category: None,
                    position: NodePosition { x: 0.0, y: 0.0 },
                    label: None,
                    params: json!({"value": 2}),
                    inputs: vec![],
                    outputs: vec![port("value", "Value", "number")],
                },
                BlueprintNode {
                    id: "n_create".into(),
                    kind: "array.create".into(),
                    category: None,
                    position: NodePosition { x: 0.0, y: 0.0 },
                    label: None,
                    params: json!({"count": 2}),
                    inputs: vec![
                        port("field_1", "Field 1", "any"),
                        port("field_2", "Field 2", "any"),
                    ],
                    outputs: vec![port("out", "Out", "array")],
                },
                BlueprintNode {
                    id: "n_add".into(),
                    kind: "math.add".into(),
                    category: None,
                    position: NodePosition { x: 0.0, y: 0.0 },
                    label: None,
                    params: json!({}),
                    inputs: vec![
                        port("a", "A", "number"),
                        port("b", "B", "number"),
                    ],
                    outputs: vec![port("sum", "Sum", "number")],
                },
                BlueprintNode {
                    id: "n_out".into(),
                    kind: "io.output".into(),
                    category: None,
                    position: NodePosition { x: 0.0, y: 0.0 },
                    label: None,
                    params: json!({"name": "result"}),
                    inputs: vec![port("in", "In", "any")],
                    outputs: vec![],
                },
            ],
            vec![
                // Matches the exact duplicate-pair shape from the
                // user-reported blueprint: each (src, tgt, port)
                // combination appears twice with different edge ids.
                BlueprintConnection {
                    id: "xy1".into(),
                    source_node: "n_lit".into(),
                    source_port: "value".into(),
                    target_node: "n_create".into(),
                    target_port: "field_1".into(),
                },
                BlueprintConnection {
                    id: "e1".into(),
                    source_node: "n_lit".into(),
                    source_port: "value".into(),
                    target_node: "n_create".into(),
                    target_port: "field_1".into(),
                },
                BlueprintConnection {
                    id: "xy2".into(),
                    source_node: "n_lit".into(),
                    source_port: "value".into(),
                    target_node: "n_create".into(),
                    target_port: "field_2".into(),
                },
                BlueprintConnection {
                    id: "e2".into(),
                    source_node: "n_lit".into(),
                    source_port: "value".into(),
                    target_node: "n_create".into(),
                    target_port: "field_2".into(),
                },
                BlueprintConnection {
                    id: "xy3".into(),
                    source_node: "n_create".into(),
                    source_port: "out".into(),
                    target_node: "n_add".into(),
                    target_port: "a".into(),
                },
                BlueprintConnection {
                    id: "e3".into(),
                    source_node: "n_create".into(),
                    source_port: "out".into(),
                    target_node: "n_add".into(),
                    target_port: "a".into(),
                },
                BlueprintConnection {
                    id: "xy4".into(),
                    source_node: "n_lit".into(),
                    source_port: "value".into(),
                    target_node: "n_add".into(),
                    target_port: "b".into(),
                },
                BlueprintConnection {
                    id: "e4".into(),
                    source_node: "n_lit".into(),
                    source_port: "value".into(),
                    target_node: "n_add".into(),
                    target_port: "b".into(),
                },
                BlueprintConnection {
                    id: "xy5".into(),
                    source_node: "n_add".into(),
                    source_port: "sum".into(),
                    target_node: "n_out".into(),
                    target_port: "in".into(),
                },
                BlueprintConnection {
                    id: "e5".into(),
                    source_node: "n_add".into(),
                    source_port: "sum".into(),
                    target_node: "n_out".into(),
                    target_port: "in".into(),
                },
            ],
        );
        let ext = HashMap::new();
        let res = run_sync(&bp, &ext).expect("DAG with duplicate edges must run");
        assert_eq!(res.stats.nodes_executed, 4);
    }

    #[test]
    fn exec_pin_chain_completes_with_mixed_exec_and_data_wires() {
        // Smoke test for the new 'exec' port shape. The blueprint
        // threads both data and exec wires through a processing node
        // (`string.length`) that exposes then_in + then_out by virtue
        // of being a processing kind. Every declared input port counts
        // in `port_pending`; this test pins that wired exec edges do
        // not crash the runner and that downstream nodes fire after
        // their upstream exec producer.
        let bp = make_blueprint(
            vec![
                BlueprintNode {
                    id: "lit".into(),
                    kind: "literal.string".into(),
                    category: None,
                    position: NodePosition { x: 0.0, y: 0.0 },
                    label: None,
                    params: json!({"value": "hello"}),
                    inputs: vec![],
                    outputs: vec![port("value", "Value", "string")],
                },
                BlueprintNode {
                    id: "len".into(),
                    kind: "string.length".into(),
                    category: None,
                    position: NodePosition { x: 0.0, y: 0.0 },
                    label: None,
                    params: json!({}),
                    inputs: vec![
                        // Catalog schema: string.length's first input
                        // is `then_in` (exec) post-migration.
                        port("then_in", "Then", "exec"),
                        port("in", "In", "string"),
                    ],
                    outputs: vec![
                        port("then_out", "Then", "exec"),
                        port("out", "Out", "number"),
                    ],
                },
                BlueprintNode {
                    id: "log".into(),
                    kind: "io.logger".into(),
                    category: None,
                    position: NodePosition { x: 0.0, y: 0.0 },
                    label: None,
                    params: json!({"label": "DONE"}),
                    inputs: vec![
                        port("then_in", "Then", "exec"),
                        port("in", "In", "any"),
                    ],
                    outputs: vec![],
                },
            ],
            vec![
                // Data wires
                BlueprintConnection {
                    id: "d1".into(),
                    source_node: "lit".into(),
                    source_port: "value".into(),
                    target_node: "len".into(),
                    target_port: "in".into(),
                },
                BlueprintConnection {
                    id: "d2".into(),
                    source_node: "len".into(),
                    source_port: "out".into(),
                    target_node: "log".into(),
                    target_port: "in".into(),
                },
                // Exec wire — length1's then_out fires only after the
                // node completes; log's then_in waits for it.
                BlueprintConnection {
                    id: "e1".into(),
                    source_node: "len".into(),
                    source_port: "then_out".into(),
                    target_node: "log".into(),
                    target_port: "then_in".into(),
                },
            ],
        );
        let ext = HashMap::new();
        let res = run_sync(&bp, &ext).expect("exec-pin blueprint must run");
        assert_eq!(res.stats.nodes_executed, 3);
        assert_eq!(res.logs.len(), 1);
        assert_eq!(res.logs[0].label, "DONE");
    }

    #[test]
    fn exec_pin_unwired_runs_on_data_alone() {
        // `then_in` declared but UNWIRED must not block — the runner
        // collapses unconnected exec edges to the same default-fill
        // (`Value::Null` / 0 pending) that every other unconnected
        // port gets. Pin so a future refactor that treats exec edges
        // specially (e.g. requiring an explicit "skip" signal) doesn't
        // accidentally lock up nodes with nothing wired to their
        // then_in slot.
        let bp = make_blueprint(
            vec![
                BlueprintNode {
                    id: "lit".into(),
                    kind: "literal.string".into(),
                    category: None,
                    position: NodePosition { x: 0.0, y: 0.0 },
                    label: None,
                    params: json!({"value": "hi"}),
                    inputs: vec![],
                    outputs: vec![port("value", "Value", "string")],
                },
                BlueprintNode {
                    id: "log".into(),
                    kind: "io.logger".into(),
                    category: None,
                    position: NodePosition { x: 0.0, y: 0.0 },
                    label: None,
                    params: json!({"label": "OK"}),
                    inputs: vec![
                        port("then_in", "Then", "exec"),
                        port("in", "In", "any"),
                    ],
                    outputs: vec![],
                },
            ],
            vec![BlueprintConnection {
                id: "d1".into(),
                source_node: "lit".into(),
                source_port: "value".into(),
                target_node: "log".into(),
                target_port: "in".into(),
            }],
        );
        let ext = HashMap::new();
        let res = run_sync(&bp, &ext).expect("unwired-then_in must not block");
        assert_eq!(res.stats.nodes_executed, 2);
        assert_eq!(res.logs.len(), 1);
        assert_eq!(res.logs[0].label, "OK");
    }

    #[test]
    fn exec_pin_overrides_alphabetical_tiebreak() {
        // The whole point of exec pins is to FORCE an ordering the
        // dataflow would otherwise leave to the alphabetical tie-break.
        // Two parallel data branches (a → log_a, z → log_z) would
        // fire in id order WITHOUT the exec wire: log_a comes before
        // log_z because a < z. We add `z.then_out → a.then_in` so
        // a must wait for z to finish, reversing the order. Without
        // the exec wire the assertion `logs[0].label == "log_z"`
        // FAILS (it'd be "log_a") — so this test pins the feature's
        // core semantic.
        let bp = make_blueprint(
            vec![
                // Independent data sources.
                BlueprintNode {
                    id: "lit1".into(),
                    kind: "literal.number".into(),
                    category: None,
                    position: NodePosition { x: 0.0, y: 0.0 },
                    label: None,
                    params: json!({"value": 1}),
                    inputs: vec![],
                    outputs: vec![port("value", "Value", "number")],
                },
                BlueprintNode {
                    id: "lit2".into(),
                    kind: "literal.number".into(),
                    category: None,
                    position: NodePosition { x: 0.0, y: 0.0 },
                    label: None,
                    params: json!({"value": 2}),
                    inputs: vec![],
                    outputs: vec![port("value", "Value", "number")],
                },
                BlueprintNode {
                    id: "lit3".into(),
                    kind: "literal.number".into(),
                    category: None,
                    position: NodePosition { x: 0.0, y: 0.0 },
                    label: None,
                    params: json!({"value": 3}),
                    inputs: vec![],
                    outputs: vec![port("value", "Value", "number")],
                },
                BlueprintNode {
                    id: "lit4".into(),
                    kind: "literal.number".into(),
                    category: None,
                    position: NodePosition { x: 0.0, y: 0.0 },
                    label: None,
                    params: json!({"value": 4}),
                    inputs: vec![],
                    outputs: vec![port("value", "Value", "number")],
                },
                // Processing nodes (math.add gets the exec-pin
                // migration, so inputs[0] is then_in and outputs[0]
                // is then_out post-catalog-change).
                BlueprintNode {
                    id: "a".into(),
                    kind: "math.add".into(),
                    category: None,
                    position: NodePosition { x: 0.0, y: 0.0 },
                    label: None,
                    params: json!({}),
                    inputs: vec![
                        port("then_in", "Then", "exec"),
                        port("a", "A", "number"),
                        port("b", "B", "number"),
                    ],
                    outputs: vec![
                        port("then_out", "Then", "exec"),
                        port("sum", "Sum", "number"),
                    ],
                },
                BlueprintNode {
                    id: "z".into(),
                    kind: "math.add".into(),
                    category: None,
                    position: NodePosition { x: 0.0, y: 0.0 },
                    label: None,
                    params: json!({}),
                    inputs: vec![
                        port("then_in", "Then", "exec"),
                        port("a", "A", "number"),
                        port("b", "B", "number"),
                    ],
                    outputs: vec![
                        port("then_out", "Then", "exec"),
                        port("sum", "Sum", "number"),
                    ],
                },
                // Sinks (io.logger has then_in only).
                BlueprintNode {
                    id: "log_a".into(),
                    kind: "io.logger".into(),
                    category: None,
                    position: NodePosition { x: 0.0, y: 0.0 },
                    label: None,
                    params: json!({"label": "log_a"}),
                    inputs: vec![
                        port("then_in", "Then", "exec"),
                        port("in", "In", "any"),
                    ],
                    outputs: vec![],
                },
                BlueprintNode {
                    id: "log_z".into(),
                    kind: "io.logger".into(),
                    category: None,
                    position: NodePosition { x: 0.0, y: 0.0 },
                    label: None,
                    params: json!({"label": "log_z"}),
                    inputs: vec![
                        port("then_in", "Then", "exec"),
                        port("in", "In", "any"),
                    ],
                    outputs: vec![],
                },
            ],
            vec![
                // Data wires
                BlueprintConnection {
                    id: "d1".into(),
                    source_node: "lit1".into(),
                    source_port: "value".into(),
                    target_node: "a".into(),
                    target_port: "a".into(),
                },
                BlueprintConnection {
                    id: "d2".into(),
                    source_node: "lit2".into(),
                    source_port: "value".into(),
                    target_node: "a".into(),
                    target_port: "b".into(),
                },
                BlueprintConnection {
                    id: "d3".into(),
                    source_node: "lit3".into(),
                    source_port: "value".into(),
                    target_node: "z".into(),
                    target_port: "a".into(),
                },
                BlueprintConnection {
                    id: "d4".into(),
                    source_node: "lit4".into(),
                    source_port: "value".into(),
                    target_node: "z".into(),
                    target_port: "b".into(),
                },
                BlueprintConnection {
                    id: "d5".into(),
                    source_node: "a".into(),
                    source_port: "sum".into(),
                    target_node: "log_a".into(),
                    target_port: "in".into(),
                },
                BlueprintConnection {
                    id: "d6".into(),
                    source_node: "z".into(),
                    source_port: "sum".into(),
                    target_node: "log_z".into(),
                    target_port: "in".into(),
                },
                // THE EXEC WIRE: forces `a` to wait for `z` to
                // finish. Without this wire, log_a fires before
                // log_z (alphabetical); with it, log_z fires first.
                BlueprintConnection {
                    id: "exec".into(),
                    source_node: "z".into(),
                    source_port: "then_out".into(),
                    target_node: "a".into(),
                    target_port: "then_in".into(),
                },
            ],
        );
        let ext = HashMap::new();
        let res = run_sync(&bp, &ext).expect("forced-order exec chain must run");
        assert_eq!(res.stats.nodes_executed, 8);
        assert_eq!(res.logs.len(), 2);
        // The exact assertion that PROVES ordering was forced:
        // log_z must be FIRST despite `z > a` alphabetically.
        //
        // This holds because `runner::topo_sort` extends the
        // frontier with newly-ready targets via
        // `frontier.extend(pushed.drain(..))` — newly-ready nodes
        // join at the END of the existing frontier WITHOUT being
        // re-sorted. log_z has only its data input wired
        // (`z.sum → log_z.in`); its `then_in` is intentionally
        // unwired, so log_z becomes ready the moment z emits sum
        // — one round BEFORE log_a is even considered. log_a only
        // becomes ready when a completes one round later and gets
        // appended after log_z in the frontier; without the exec
        // wire on a.then_in, both loggers would join in the same
        // round and alphabetical would put log_a first. If a future
        // refactor switches the frontier to a sorted collection
        // (BTreeSet, or a post-insert sort), log_a would win the
        // alphabetical tie-break and this assertion would have
        // to flip.
        assert_eq!(
            res.logs[0].label, "log_z",
            "exec-pin chain must force log_z before log_a despite alphabetical tie-break"
        );
        assert_eq!(res.logs[1].label, "log_a");
    }

    #[test]
    fn inline_default_used_when_port_unwired() {
        // Blender-style "input deflection" UX: every scalar input
        // gains an editable default on the canvas, persisted as
        // `params[<port_id>]`. The runner falls back to that value
        // whenever the port is unwired. Topology: lit2 → add.b; add.a
        // is unwired; out gets add.sum. Add's `params.a = 7` should
        // thread through — 7 + 3 (lit2.value) = 10 on `result`.
        let bp = make_blueprint(
            vec![
                BlueprintNode {
                    id: "lit2".into(),
                    kind: "literal.number".into(),
                    category: None,
                    position: NodePosition { x: 0.0, y: 0.0 },
                    label: None,
                    params: json!({"value": 3}),
                    inputs: vec![],
                    outputs: vec![port("value", "Value", "number")],
                },
                BlueprintNode {
                    id: "add".into(),
                    kind: "math.add".into(),
                    category: None,
                    position: NodePosition { x: 0.0, y: 0.0 },
                    label: None,
                    // Inline default typed in the canvas — only port
                    // `a` is set; port `b` falls back to whatever
                    // gets wired.
                    params: json!({"a": 7}),
                    inputs: vec![
                        port("then_in", "Then", "exec"),
                        port("a", "A", "number"),
                        port("b", "B", "number"),
                    ],
                    outputs: vec![
                        port("then_out", "Then", "exec"),
                        port("sum", "Sum", "number"),
                    ],
                },
                BlueprintNode {
                    id: "out".into(),
                    kind: "io.output".into(),
                    category: None,
                    position: NodePosition { x: 0.0, y: 0.0 },
                    label: None,
                    params: json!({"name": "result"}),
                    inputs: vec![
                        port("then_in", "Then", "exec"),
                        port("in", "In", "any"),
                    ],
                    outputs: vec![],
                },
            ],
            vec![
                BlueprintConnection {
                    id: "d1".into(),
                    source_node: "lit2".into(),
                    source_port: "value".into(),
                    target_node: "add".into(),
                    target_port: "b".into(),
                },
                BlueprintConnection {
                    id: "d2".into(),
                    source_node: "add".into(),
                    source_port: "sum".into(),
                    target_node: "out".into(),
                    target_port: "in".into(),
                },
            ],
        );
        let ext = HashMap::new();
        let res = run_sync(&bp, &ext).expect("inline-default fallback must run");
        assert_eq!(
            res.outputs.get("result").and_then(|v| v.as_f64()),
            Some(10.0),
            "params.a=7 + lit2.value=3 must yield 10; proves runner-level inline default"
        );
    }

    #[test]
    fn inline_default_overridden_by_wired_value() {
        // Same shape as above but BOTH `a` and `b` are wired. The
        // wire must override params.a (the whole point of input
        // deflection: wire always wins when connected and
        // non-Null).
        let bp = make_blueprint(
            vec![
                BlueprintNode {
                    id: "lit_a".into(),
                    kind: "literal.number".into(),
                    category: None,
                    position: NodePosition { x: 0.0, y: 0.0 },
                    label: None,
                    params: json!({"value": 100}),
                    inputs: vec![],
                    outputs: vec![port("value", "Value", "number")],
                },
                BlueprintNode {
                    id: "lit_b".into(),
                    kind: "literal.number".into(),
                    category: None,
                    position: NodePosition { x: 0.0, y: 0.0 },
                    label: None,
                    params: json!({"value": 100}),
                    inputs: vec![],
                    outputs: vec![port("value", "Value", "number")],
                },
                BlueprintNode {
                    id: "add".into(),
                    kind: "math.add".into(),
                    category: None,
                    position: NodePosition { x: 0.0, y: 0.0 },
                    label: None,
                    // params.a=7 should be IGNORED — both ports wired.
                    params: json!({"a": 7}),
                    inputs: vec![
                        port("then_in", "Then", "exec"),
                        port("a", "A", "number"),
                        port("b", "B", "number"),
                    ],
                    outputs: vec![
                        port("then_out", "Then", "exec"),
                        port("sum", "Sum", "number"),
                    ],
                },
                BlueprintNode {
                    id: "out".into(),
                    kind: "io.output".into(),
                    category: None,
                    position: NodePosition { x: 0.0, y: 0.0 },
                    label: None,
                    params: json!({"name": "result"}),
                    inputs: vec![
                        port("then_in", "Then", "exec"),
                        port("in", "In", "any"),
                    ],
                    outputs: vec![],
                },
            ],
            vec![
                BlueprintConnection {
                    id: "d1".into(),
                    source_node: "lit_a".into(),
                    source_port: "value".into(),
                    target_node: "add".into(),
                    target_port: "a".into(),
                },
                BlueprintConnection {
                    id: "d2".into(),
                    source_node: "lit_b".into(),
                    source_port: "value".into(),
                    target_node: "add".into(),
                    target_port: "b".into(),
                },
                BlueprintConnection {
                    id: "d3".into(),
                    source_node: "add".into(),
                    source_port: "sum".into(),
                    target_node: "out".into(),
                    target_port: "in".into(),
                },
            ],
        );
        let ext = HashMap::new();
        let res = run_sync(&bp, &ext).expect("wired-overrides-default must run");
        // 100 (lit_a → a) + 100 (lit_b → b). params.a=7 ignored.
        assert_eq!(
            res.outputs.get("result").and_then(|v| v.as_f64()),
            Some(200.0),
            "wired values must override params.a=7; lit_a=100 + lit_b=100 = 200"
        );
    }

    #[test]
    fn logger_runs_first_when_sibling_is_alphabetically_earlier() {
        // Two siblings of the same upstream `lit`: `out` (whose id
        // is `a` so without the priority class the alphabetical
        // tie-break would run it first) and `log` (whose id is `z`
        // so without the priority class it would run second). The
        // logger-first rule in `topo_sort` prioritises by node kind
        // BEFORE the alphabetical fallback: `log` is `io.logger`,
        // `out` is `io.output`, so `log` runs first even though
        // `z > a` lexically. Pin the user's "logger antes que el
        // resto para debuggear" contract — without this rule the
        // assertion below would flip.
        let bp = make_blueprint(
            vec![
                BlueprintNode {
                    id: "lit".into(),
                    kind: "literal.number".into(),
                    category: None,
                    position: NodePosition { x: 0.0, y: 0.0 },
                    label: None,
                    params: json!({"value": 42}),
                    inputs: vec![],
                    outputs: vec![port("value", "Value", "number")],
                },
                BlueprintNode {
                    id: "a".into(),
                    kind: "io.output".into(),
                    category: None,
                    position: NodePosition { x: 0.0, y: 0.0 },
                    label: None,
                    // `name` is the output key on the run result;
                    // we don't read it here, only `stats.nodes_executed`.
                    params: json!({"name": "result"}),
                    inputs: vec![
                        port("then_in", "Then", "exec"),
                        port("in", "In", "any"),
                    ],
                    outputs: vec![],
                },
                BlueprintNode {
                    id: "z".into(),
                    kind: "io.logger".into(),
                    category: None,
                    position: NodePosition { x: 0.0, y: 0.0 },
                    label: None,
                    params: json!({"label": "log"}),
                    inputs: vec![
                        port("then_in", "Then", "exec"),
                        port("in", "In", "any"),
                    ],
                    outputs: vec![],
                },
            ],
            vec![BlueprintConnection {
                id: "e1".into(),
                source_node: "lit".into(),
                source_port: "value".into(),
                target_node: "a".into(),
                target_port: "in".into(),
            },
            BlueprintConnection {
                id: "e2".into(),
                source_node: "lit".into(),
                source_port: "value".into(),
                target_node: "z".into(),
                target_port: "in".into(),
            }],
        );
        let ext = HashMap::new();
        let res = run_sync(&bp, &ext)
            .expect("two-sibling logger-priority DAG must run");
        // `log_z` is the second node executed; `a` is third. The
        // `a < z` alphabetical tie-break alone would have produced
        // `a` SECOND and `z` third — the priority class flipped it.
        assert_eq!(res.stats.nodes_executed, 3);
        assert_eq!(res.logs.len(), 1);
        assert_eq!(
            res.logs[0].label, "log",
            "logger must run before its alphabetically-earlier sibling"
        );
    }

    #[test]
    fn run_graph_detects_cycle() {
        let bp = make_blueprint(
            vec![
                BlueprintNode {
                    id: "a".into(),
                    kind: "literal.number".into(),
                    category: None,
                    position: NodePosition { x: 0.0, y: 0.0 },
                    label: None,
                    params: json!({"value": 1}),
                    inputs: vec![],
                    outputs: vec![port("value", "Value", "number")],
                },
                BlueprintNode {
                    id: "b".into(),
                    kind: "math.add".into(),
                    category: None,
                    position: NodePosition { x: 0.0, y: 0.0 },
                    label: None,
                    params: json!({}),
                    inputs: vec![port("a", "A", "number"), port("b", "B", "number")],
                    outputs: vec![port("sum", "Sum", "number")],
                },
            ],
            vec![
                BlueprintConnection {
                    id: "e1".into(),
                    source_node: "a".into(),
                    source_port: "value".into(),
                    target_node: "b".into(),
                    target_port: "a".into(),
                },
                BlueprintConnection {
                    id: "e2".into(),
                    source_node: "b".into(),
                    source_port: "sum".into(),
                    target_node: "a".into(),
                    target_port: "b".into(),
                },
            ],
        );
        let ext = HashMap::new();
        let err = run_sync(&bp, &ext).unwrap_err();
        assert!(matches!(err.error, ExecError::Cycle(_)));
    }
}
