// Helpers that convert between Svelte Flow's runtime graph state and
// our on-disk Blueprint JSON format. The flow shape mirrors what Svelte
// Flow consumes/produces; the Blueprint shape mirrors src-tauri/blueprint.rs.

import type { Node, Edge, Connection } from '@xyflow/svelte';
import { getDef, defaultParams, resolvedRepeatCount, synthInputs } from '../nodes/catalog';
import type {
  Port,
  PortDataType,
  Blueprint,
  BlueprintMeta,
  BlueprintInput,
} from '../types/blueprint';
import {
  SCHEMA_VERSION,
  migrateBlueprintInput,
} from '../types/blueprint';

/**
 * Concrete flow-data shape every node carries. The extra `[key: string]:
 * unknown` index signature satisfies Svelte Flow's `Record<string,
 * unknown>` constraint on `Node<T>` without forcing every call-site to
 * reflect on structural typing.
 */
export interface NodeData {
  kind: string;
  category?: string;
  label?: string;
  params: Record<string, unknown>;
  inputs: Port[];
  outputs: Port[];
  [key: string]: unknown;
}

export type FlowNode = Node<NodeData>;
export type FlowEdge = Edge;

/**
 * Map a `BlueprintInput.type` declaration to the wire-level
 * `PortDataType` that flows through the canvas.
 *
 * Rules (mirror how the `file.pathPicker` node already wraps the
 * `tauri-plugin-dialog` modes):
 *   * `file`       → single string path   → `string` port
 *   * `folder`     → single string path   → `string` port
 *   * `multi-file` → array of string paths → `array` port
 *   * `string` / `number` / `boolean` / `array` / `any` pass through verbatim
 *
 * Anything else falls back to `any` so a malformed blueprint file
 * can never crash the editor.
 *
 * Used by:
 *   * `portFromBlueprintInput` (saved ports + visual ports for blueprint.input nodes)
 *   * `+page.svelte` `inputRequests` (label passed to InputsDialog)
 *   * `dashboard/+page.svelte` `inputRequests` (Run-from-dashboard modal)
 */
export function portTypeFromInputType(inpType: string): PortDataType {
  switch (inpType) {
    case 'file':
    case 'folder':
    case 'string':
      return 'string';
    case 'multi-file':
    case 'array':
      return 'array';
    case 'number':
      return 'number';
    case 'boolean':
      return 'boolean';
    case 'any':
      return 'any';
    default:
      return 'any';
  }
}

/**
 * Convert a single `BlueprintInput` declaration into a `Port` for
 * Svelte Flow rendering (and for the on-disk saved shape). Used in BOTH
 * `composeBlueprint` (writing the saved Blueprint file) and
 * `blueprintToFlow` (loading one into the editor) so the two paths
 * can't drift.
 */
export function portFromBlueprintInput(inp: BlueprintInput): Port {
  return {
    id: inp.id,
    name: inp.name,
    type: portTypeFromInputType(inp.type),
  };
}

/** Compose a complete Blueprint (for file persistence) from meta + graph. */
export function composeBlueprint(
  meta: BlueprintMeta,
  nodes: FlowNode[],
  edges: FlowEdge[],
): Blueprint {
  const bpNodes = nodes.map((n) => {
    const def = getDef(String(n.data.kind));
    const params = { ...(n.data.params ?? {}) };
    const kind = String(n.data.kind ?? 'unknown');
    // For repeatable kinds, regenerate the persisted `inputs` list from
    // the catalog's `repeatableInputs + params.count`. Authoritative
    // because the `inspector`-editable count is the source of truth at
    // runtime; we don't trust any `n.data.inputs` that might be stale.
    let inputs: Port[];
    if (def?.repeatableInputs) {
      const count = resolvedRepeatCount(def, params) ?? def.repeatableInputs.min;
      inputs = synthInputs(def, count) ?? [];
    } else {
      inputs = (n.data.inputs ?? []).map((p) => ({ ...p }));
    }
    return {
      id: n.id,
      kind,
      category: n.data.category,
      position: { x: n.position.x, y: n.position.y },
      label: n.data.label,
      params,
      inputs,
      // `blueprint.input` nodes share the output-port schema with the
      // parent blueprint's `metadata.inputs` (Blender group-input
      // semantics). Regenerate from `meta.metadata.inputs` so the saved
      // file is self-describing and a JSON diff is honest about the
      // port set; at run-time the Rust executor reads metadata
      // directly so the on-disk outputs are informational but useful.
      //
      // `BlueprintInput.type` may carry the picker kinds (`file`,
      // `folder`, `multi-file`) which map to wire-level scalar/array
      // types via `portTypeFromInputType`. We also run the legacy
      // `'path' → 'file'` migration on each input so a Blueprint
      // authored before the picker split still serialises back to a
      // current-schema shape (instead of writing the deprecated
      // `'path'` to disk on Save).
      outputs: kind === 'blueprint.input'
        ? meta.metadata.inputs.map((i) => portFromBlueprintInput(migrateBlueprintInput(i)))
        : (n.data.outputs ?? []).map((p) => ({ ...p })),
    };
  });

  // Build a per-node admissible target-port lookup so we can drop
  // orphan edges. Today only `target` can go orphan (`array.create`
  // shrinks its count and any edge into `field_5`+ becomes lonely);
  // sources are fixed per kind so no orphan there. Future
  // `repeatableOutputs` would extend this similarly.
  const targetPortByNode = new Map<string, Set<string>>();
  for (const n of bpNodes) {
    targetPortByNode.set(n.id, new Set(n.inputs.map((p) => p.id)));
  }

  const bpEdges: Blueprint['connections'] = edges
    .filter(
      (e) =>
        !!e.sourceHandle &&
        !!e.targetHandle &&
        targetPortByNode.get(e.target)?.has((e.targetHandle ?? '') as string),
    )
    .map((e) => ({
      id: e.id,
      sourceNode: e.source,
      sourcePort: (e.sourceHandle ?? '') as string,
      targetNode: e.target,
      targetPort: (e.targetHandle ?? '') as string,
    }));

  return {
    schema_version: SCHEMA_VERSION,
    id: meta.id,
    name: meta.name,
    description: meta.description,
    created_at: meta.created_at,
    updated_at: new Date().toISOString(),
    nodes: bpNodes,
    connections: bpEdges,
    metadata: meta.metadata,
  };
}

/** Build Svelte Flow nodes/edges from a loaded Blueprint. */
export function blueprintToFlow(bp: Blueprint): { nodes: FlowNode[]; edges: FlowEdge[] } {
  const nodes: FlowNode[] = bp.nodes.map((n) => {
    const def = getDef(n.kind);
    const params = def ? { ...defaultParams(def), ...n.params } : { ...n.params };
    // `blueprint.input` outputs are owned by `bp.metadata.inputs`,
    // not by the saved `n.outputs` (which may be stale if the blueprint
    // was authored when the input list differed). Re-derive on load so
    // the editor visualisation matches the runtime contract. We also
    // run the legacy `'path' → 'file'` migration here so even
    // blueprints loaded outside of `stripMeta` (e.g. by the
    // dashboard's handleRun path) display the picker-kind-correct
    // ports on the canvas.
    const outputs: Port[] = n.kind === 'blueprint.input'
      ? bp.metadata.inputs.map((i) => portFromBlueprintInput(migrateBlueprintInput(i)))
      : n.outputs;
    const data: NodeData = {
      kind: n.kind,
      category: n.category ?? def?.category,
      label: n.label,
      params,
      inputs: n.inputs,
      outputs,
    };
    return {
      id: n.id,
      type: 'default',
      position: { x: n.position.x, y: n.position.y },
      data,
    };
  });

  const edges: FlowEdge[] = bp.connections.map((c) => ({
    id: c.id,
    source: c.sourceNode,
    target: c.targetNode,
    sourceHandle: c.sourcePort,
    targetHandle: c.targetPort,
  }));

  return { nodes, edges };
}

/** Create a fresh Svelte Flow node for the given kind at the given position. */
export function makeFlowNode(kind: string, position: { x: number; y: number }): FlowNode {
  const def = getDef(kind);
  if (!def) throw new Error(`Unknown node kind: ${kind}`);
  const id =
    typeof crypto !== 'undefined' && 'randomUUID' in crypto
      ? (crypto.randomUUID() as string)
      : `n_${Math.random().toString(36).slice(2, 10)}`;

  const data: NodeData = {
    kind,
    category: def.category,
    label: def.label,
    params: defaultParams(def),
    // Leave inputs empty for repeatable kinds; BlueprintNode renders them
    // from `def.repeatableInputs + params.count` on the fly.
    inputs: def.repeatableInputs ? [] : def.inputs.map((p) => ({ ...p })),
    outputs: def.outputs.map((p) => ({ ...p })),
  };

  return { id, type: 'default', position, data };
}

/** Quick ID generator suitable for edges. */
export function newEdgeId(source: string, target: string): string {
  return `e_${source}__${target}__${Math.random().toString(36).slice(2, 8)}`;
}

/** Build an edge from a Svelte Flow connection event. */
export function edgeFromConnection(conn: Connection): FlowEdge {
  return {
    id: newEdgeId(conn.source, conn.target),
    source: conn.source,
    target: conn.target,
    sourceHandle: conn.sourceHandle,
    targetHandle: conn.targetHandle,
  };
}

// Note: `stripMeta` lives in `$lib/types/blueprint.ts` (canonical).
