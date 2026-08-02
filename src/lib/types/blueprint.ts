// Blueprint on-disk JSON schema. Mirrors src-tauri/src/blueprint.rs.
// Keep these two definitions in sync when introducing a new field.

export const SCHEMA_VERSION = 1 as const;

export type PortDataType =
  | 'number'
  | 'string'
  | 'boolean'
  | 'array'
  | 'any'
  | 'binary'
  /** Visual type for ports that explicitly accept either a scalar
   *  string OR a string array. Distinguishes shape-mirroring ports
   *  (e.g. `string.replace.in`, `file.delete.paths`) from nominally-
   *  typed ports that just happen to also work in both shapes via
   *  weak coercion. The runtime executor is unaffected — it always
   *  branches on `Value::String` vs `Value::Array` regardless of
   *  how the catalog tags the port. */
  | 'string-or-array'
  /** Execution / control-flow token. Solid black circle on the
   *  canvas (matches the `number` shape; distinguished in TWO
   *  ways by intent — colour is `#000000` vs the palette-coloured
   *  data pins, AND the white inner border that every `.port`
   *  ships with is explicitly stripped so the exec fill reads as a
   *  solid mass): ports of this type carry NO data, only an
   *  ordering signal. A `then_out` port emits a Null token after
   *  the executor finishes the node; a `then_in` port reads it
   *  purely to participate in topological order — its value is
   *  ignored by the executor. Wiring rules in `Editor.isValidConnection`
   *  enforce that two `exec` ports may wire to each other but a
   *  `then_out` may NOT land on a data input (and vice versa). */
  | 'exec';

export interface Port {
  id: string;
  name: string;
  type: PortDataType;
}

/**
 * A single user-declared input on a blueprint. The canonical list
 * lives on `BlueprintMetadata.inputs` and is shared across every
 * `blueprint.input` node in the graph — Blender group-input semantics,
 * where the group interface is one thing and every group node in the
 * graph surfaces it.
 *
 * The `type` field is informational and used for input-modal rendering
 * on the frontend. At run-time the executor reads `external_inputs`
 * keyed by `id` regardless of declared type, so a wrong type on disk
 * is a user-misconfiguration, not a crash.
 */
export interface BlueprintInput {
  /** Stable, unique id used as the runtime key. Must match `^[a-zA-Z0-9_]+$`. */
  id: string;
  /** Human label for the dashboard modal and palette tooltip. */
  name: string;
  /**
   * `"string" | "number" | "boolean" | "file" | "folder" | "multi-file" | "array" | "any"`.
   *
   * The three picker-style types hint at how the InputsDialog should
   * collect the value when the user runs the blueprint:
   *   * `file`       → single-path text + Browse… (native file picker).
   *   * `folder`     → single-path text + Browse… (native folder picker).
   *   * `multi-file` → chip list + Add files… (native multi-select).
   *
   * At run-time the value flowing through the wire IS a string for
   * `file`/`folder` and a `string[]` for `multi-file`. `portFromBlueprintInput`
   * maps these to the corresponding `PortDataType` (`string` for the
   * single-file types, `array` for multi-file).
   */
  type: 'string' | 'number' | 'boolean' | 'file' | 'folder' | 'multi-file' | 'array' | 'any';
  /** Optional default. The runner pre-fills `external_inputs[id]` if the
   *  user supplies no value at run-time. */
  default?: unknown;
  /** Free-text description shown next to the field in the dashboard
   *  modal. */
  description?: string;
}

export interface BlueprintNode {
  id: string;
  /** Stable kind id (e.g. `math.add`). */
  kind: string;
  /** Group for palette, e.g. `Math`, `String`, `IO`. */
  category?: string;
  position: { x: number; y: number };
  label?: string;
  /** Node-specific configuration. Shape depends on `kind`. */
  params: Record<string, unknown>;
  inputs: Port[];
  outputs: Port[];
}

export interface BlueprintConnection {
  id: string;
  sourceNode: string;
  sourcePort: string;
  targetNode: string;
  targetPort: string;
}

export interface BlueprintMetadata {
  author?: string;
  tags: string[];
  /** Blender-style inputs. Every `blueprint.input` node in the graph
   *  surfaces this set as its output ports. */
  inputs: BlueprintInput[];
}

export interface Blueprint {
  schema_version: typeof SCHEMA_VERSION;
  id: string;
  name: string;
  description: string;
  created_at?: string;
  updated_at?: string;
  nodes: BlueprintNode[];
  connections: BlueprintConnection[];
  metadata: BlueprintMetadata;
}

/**
 * Subset of the Blueprint we hold as "metadata" in client state.
 * The graph itself (nodes, connections) lives in Svelte Flow runtime
 * state so the user can drag/connect without round-tripping through
 * a serializable snapshot.
 */
export interface BlueprintMeta {
  id: string;
  name: string;
  description: string;
  created_at?: string;
  updated_at?: string;
  schema_version: typeof SCHEMA_VERSION;
  metadata: BlueprintMetadata;
}

/** Display color per data type. Blender-shader-editor inspired. */
export const PORT_COLORS: Record<PortDataType, string> = {
  number: '#3b82f6',   // blue
  string: '#22c55e',   // green
  boolean: '#f59e0b',  // amber
  array: '#ec4899',    // pink
  any: '#6b7280',      // gray
  binary: '#8b5cf6',   // violet — distinct from 'any' gray and 'array' pink
  // Teal sits visually between string-green and array-pink on the
  // color wheel, which mirrors its semantic "either of those two"
  // role. Avoids the boolean-amber (#f59e0b, too close) and the
  // number-blue/binary-violet cluster.
  'string-or-array': '#0d9488',
  // Solid black — matches the rendered `.port-exec` circle in
  // `BlueprintNode.svelte`, which is a fully black filled handle
  // (no border, full radius). The Inspector swatch and the actual
  // canvas pin therefore look identical at a glance. Keep both
  // rules in lock-step if this colour ever changes.
  exec: '#000000',
};

export const PORT_LABELS: Record<PortDataType, string> = {
  number: 'Number',
  string: 'String',
  boolean: 'Boolean',
  array: 'Array',
  any: 'Any',
  binary: 'Binary',
  'string-or-array': 'String or Array',
  exec: 'Then',
};

/** Empty metadata envelope — used by "New" and as the initial state. */
export function makeMeta(name: string): BlueprintMeta {
  const now = new Date().toISOString();
  const id =
    typeof crypto !== 'undefined' && 'randomUUID' in crypto
      ? (crypto.randomUUID() as string)
      : `bp_${Date.now().toString(36)}`;
  return {
    id,
    name,
    description: '',
    created_at: now,
    updated_at: now,
    schema_version: SCHEMA_VERSION,
    metadata: { tags: [], inputs: [] },
  };
}

/**
 * Looser structural shape that widens `BlueprintInput.type` from the
 * narrow on-disk union to plain `string`. Lets
 * `migrateBlueprintInput` compare against the legacy `'path'` literal
 * without a per-call `as string` cast (a cast at the comparison
 * site would raise a yellow flag for future contributors, since the
 * narrow union makes `'path'` look a-priori unreachable).
 * The rest of the shape matches `BlueprintInput` so callers passing
 * current-schema values satisfy the parameter without coercion.
 */
type BlueprintInputLegacy = Omit<BlueprintInput, 'type'> & { type: string };

/**
 * Migrate a single `BlueprintInput` from a prior on-disk schema to
 * the current one. Today only `type === 'path'` needs renaming — in
 * v0 it was a generic "single path" placeholder; the current
 * schema distinguishes `file` vs `folder` vs `multi-file`.
 * Idempotent: a current-schema input returns unchanged.
 *
 * Routed through `stripMeta` and `composeBlueprint` so any code
 * that reads `meta.metadata.inputs` from a loaded Blueprint sees
 * only current-schema values, and Inspector's `<select>` always
 * renders a matching option (otherwise the visual selection
 * silently falls back to the first option while state keeps the
 * legacy value).
 */
export function migrateBlueprintInput(inp: BlueprintInputLegacy): BlueprintInput {
  // The on-disk Blueprints authored before the picker split may
  // still carry the legacy `'path'` type. This helper is the SINGLE
  // point of coercion at the load boundary: everything else in the
  // codebase reads `BlueprintInput['type']` directly.
  if (inp.type === 'path') {
    return { ...inp, type: 'file' };
  }
  return inp as BlueprintInput;
}

/** Identical-to-input migration for input lists. */
export function migrateBlueprintInputs(
  list: readonly BlueprintInput[],
): BlueprintInput[] {
  return list.map(migrateBlueprintInput);
}

/** Pull the metadata envelope out of a full Blueprint. Migrates the
 *  legacy `'path'` input kind to `'file'` on the way out so any
 *  downstream reader (Inspector, page bpMeta, InputsDialog bindings)
 *  sees a current-schema value. */
export function stripMeta(bp: Blueprint): BlueprintMeta {
  return {
    id: bp.id,
    name: bp.name,
    description: bp.description,
    created_at: bp.created_at,
    updated_at: bp.updated_at,
    schema_version: bp.schema_version,
    metadata: {
      ...bp.metadata,
      inputs: migrateBlueprintInputs(bp.metadata.inputs),
    },
  };
}
