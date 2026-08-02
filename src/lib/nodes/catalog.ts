// Node catalog — single source of truth for every node kind the user can drag in.
// Adding a new node kind is done by:
//   1. Adding an entry here (kind, category, ports, defaults).
//   2. Registering the executor in src-tauri/src/executor/nodes/mod.rs.
//   3. Optionally tweaking the renderer in lib/components/nodes/BlueprintNode.svelte
//      (today the renderer is one component that handles every kind via a kind switch).

import type { PortDataType, Port } from '../types/blueprint';

export type NodeCategory =
  | 'Literal'
  | 'Math'
  | 'String'
  | 'Array'
  | 'IO'
  | 'Json'
  | 'Image';

export const CATEGORY_COLORS: Record<NodeCategory, string> = {
  Literal: '#a855f7', // violet
  Math: '#06b6d4',    // cyan
  String: '#22c55e',  // green
  Array: '#ec4899',   // pink
  IO: '#f59e0b',      // amber
  Json: '#eab308',    // yellow
  Image: '#8b5cf6',   // violet — binary pipeline's home category
};

export type ParamField =
  | { kind: 'number'; key: string; label: string; default: number; min?: number; max?: number; step?: number }
  | { kind: 'string'; key: string; label: string; default: string; placeholder?: string }
  | { kind: 'boolean'; key: string; label: string; default: boolean }
  | { kind: 'select'; key: string; label: string; default: string; options: string[] };

/**
 * Optional declaration of N synthesised input ports whose count is
 * runtime-driven by a `count` param. When present:
 *  - `inputs: []` is expected for the kind.
 *  - `params` should include a number param whose `key` matches
 *    `countKey` (defaults to `"count"`).
 *  - `BlueprintNode.svelte` renders `prefix_1` … `prefix_N` handles plus
 *    `+`/`−` buttons in the body.
 *  - `composeBlueprint` regenerates the persisted `inputs` from
 *    `${prefix}_${i}` ids so files always round-trip cleanly.
 */
export interface RepeatableInputsConfig {
  /** id prefix, e.g. "field" → field_1, field_2, … */
  prefix: string;
  /** Display label per port, e.g. "Field" → "Field 1", "Field 2". */
  label: string;
  /** Type of every synthesised input port. */
  type: PortDataType;
  /** Minimum allowed count (typically 1). */
  min: number;
  /** Maximum allowed count — caps the `+` button. */
  max: number;
  /** Param key holding the live count. Defaults to "count". */
  countKey?: string;
}

export interface NodeDef {
  kind: string;
  category: NodeCategory;
  label: string;
  description: string;
  inputs: Array<Omit<Port, 'type'> & { type: PortDataType }>;
  outputs: Array<Omit<Port, 'type'> & { type: PortDataType }>;
  params: ParamField[];
  /** When set, the node has N runtime-driven inputs instead of fixed-shape ones. */
  repeatableInputs?: RepeatableInputsConfig;
}

// ---------- `string-or-array` migration note ----------
// Backwards-compat: blueprints authored before this PR load with their
// legacy single-type port (`string` / `array` / `binary`) intact. To
// pick up the new `string-or-array` visual type, the user re-authors the
// affected node — there is no automatic re-typing on load.
// A `binary-or-array` sibling is intended (see inline `// TODO` markers
// on binary-shaped ports that shape-mirror), but it requires parallel
// entries in `PORT_COLORS` / `PORT_LABELS` / CSS and is deferred.

const T = {
  number: 'number',
  string: 'string',
  boolean: 'boolean',
  array: 'array',
  any: 'any',
  binary: 'binary',
  // Used for ports that explicitly accept BOTH a scalar string AND
  // a string array at runtime (e.g. `string.replace.in`,
  // `file.delete.paths`, `file.saveBinary.names`). The runtime
  // executor branches on `Value::String` vs `Value::Array` already;
  // this type merely flags the dual-shape contract on the canvas so
  // the user sees a distinctive teal pentagon instead of either a
  // solid string-green square or a string-typed port that secretly
  // also handles arrays.
  //
  // Bracket-notation because TS's `satisfies Record<PortDataType,
  // PortDataType>` enforces that the OBJECT KEY must itself be a
  // member of the PortDataType union — an identifier like
  // `stringOrArray` would be an excess property. Using `'string-or-array'`
  // as the literal key keeps the satisfies check sound. Access
  // sites read it back via `T['string-or-array']`.
  ['string-or-array']: 'string-or-array',
  // Execution / control-flow port — Unreal/Godot-style white chevron
  // that carries ordering only, no data. Access site uses `T.exec`.
  exec: 'exec',
} as const satisfies Record<PortDataType, PortDataType>;

export const NODE_CATALOG: NodeDef[] = [
  // ---------- Literals ----------
  {
    kind: 'literal.number',
    category: 'Literal',
    label: 'Number',
    description: 'A constant numeric value.',
    inputs: [],
    outputs: [{ id: 'value', name: 'Value', type: T.number }],
    params: [{ kind: 'number', key: 'value', label: 'Value', default: 0, step: 0.1 }],
  },
  {
    kind: 'literal.string',
    category: 'Literal',
    label: 'String',
    description: 'A constant string value.',
    inputs: [],
    outputs: [{ id: 'value', name: 'Value', type: T.string }],
    params: [{ kind: 'string', key: 'value', label: 'Value', default: '', placeholder: 'hello…' }],
  },
  {
    kind: 'literal.boolean',
    category: 'Literal',
    label: 'Boolean',
    description: 'A constant true/false value.',
    inputs: [],
    outputs: [{ id: 'value', name: 'Value', type: T.boolean }],
    params: [{ kind: 'boolean', key: 'value', label: 'Value', default: false }],
  },
  {
    kind: 'literal.json',
    category: 'Literal',
    label: 'JSON',
    description: 'An arbitrary JSON literal (object / array / scalar).',
    inputs: [],
    outputs: [{ id: 'value', name: 'Value', type: T.any }],
    params: [{ kind: 'string', key: 'value', label: 'JSON', default: 'null', placeholder: '{} or [] or null' }],
  },

  // ---------- Math ----------
  {
    kind: 'math.add',
    category: 'Math',
    label: 'Add',
    description: 'A + B.',
    inputs: [{ id: 'then_in', name: 'Then', type: T.exec }, { id: 'a', name: 'A', type: T.number }, { id: 'b', name: 'B', type: T.number }],
    outputs: [{ id: 'then_out', name: 'Then', type: T.exec }, { id: 'sum', name: 'Sum', type: T.number }],
    params: [],
  },
  {
    kind: 'math.subtract',
    category: 'Math',
    label: 'Subtract',
    description: 'A − B.',
    inputs: [{ id: 'then_in', name: 'Then', type: T.exec }, { id: 'a', name: 'A', type: T.number }, { id: 'b', name: 'B', type: T.number }],
    outputs: [{ id: 'then_out', name: 'Then', type: T.exec }, { id: 'diff', name: 'Diff', type: T.number }],
    params: [],
  },
  {
    kind: 'math.multiply',
    category: 'Math',
    label: 'Multiply',
    description: 'A × B.',
    inputs: [{ id: 'then_in', name: 'Then', type: T.exec }, { id: 'a', name: 'A', type: T.number }, { id: 'b', name: 'B', type: T.number }],
    outputs: [{ id: 'then_out', name: 'Then', type: T.exec }, { id: 'product', name: 'Product', type: T.number }],
    params: [],
  },
  {
    kind: 'math.divide',
    category: 'Math',
    label: 'Divide',
    description: 'A ÷ B.',
    inputs: [{ id: 'then_in', name: 'Then', type: T.exec }, { id: 'a', name: 'A', type: T.number }, { id: 'b', name: 'B', type: T.number }],
    outputs: [{ id: 'then_out', name: 'Then', type: T.exec }, { id: 'quotient', name: 'Quotient', type: T.number }],
    params: [],
  },
  {
    kind: 'math.compare',
    category: 'Math',
    label: 'Compare',
    description: 'Pick an operation comparing two numbers.',
    inputs: [{ id: 'then_in', name: 'Then', type: T.exec }, { id: 'a', name: 'A', type: T.number }, { id: 'b', name: 'B', type: T.number }],
    outputs: [{ id: 'then_out', name: 'Then', type: T.exec }, { id: 'result', name: 'Result', type: T.boolean }],
    params: [
      { kind: 'select', key: 'op', label: 'Operation', default: 'eq', options: ['eq', 'neq', 'gt', 'gte', 'lt', 'lte'] },
    ],
  },

  // ---------- String ----------
  {
    kind: 'string.concat',
    category: 'String',
    label: 'Concat',
    description: 'Join two strings into one.',
    inputs: [{ id: 'then_in', name: 'Then', type: T.exec }, { id: 'a', name: 'A', type: T.string }, { id: 'b', name: 'B', type: T.string }],
    outputs: [{ id: 'then_out', name: 'Then', type: T.exec }, { id: 'out', name: 'Out', type: T.string }],
    params: [],
  },
  {
    kind: 'string.length',
    category: 'String',
    label: 'Length',
    description: 'Number of characters in a string.',
    inputs: [{ id: 'then_in', name: 'Then', type: T.exec }, { id: 'in', name: 'In', type: T.string }],
    outputs: [{ id: 'then_out', name: 'Then', type: T.exec }, { id: 'out', name: 'Length', type: T.number }],
    params: [],
  },
  {
    kind: 'string.split',
    category: 'String',
    label: 'Split',
    description: 'Split a string by a separator into an array.',
    inputs: [{ id: 'then_in', name: 'Then', type: T.exec }, { id: 'in', name: 'In', type: T.string }],
    outputs: [{ id: 'then_out', name: 'Then', type: T.exec }, { id: 'out', name: 'Out', type: T.array }],
    params: [{ kind: 'string', key: 'sep', label: 'Separator', default: ',', placeholder: ',' }],
  },
  {
    kind: 'string.join',
    category: 'String',
    label: 'Join',
    description: 'Join an array of strings with a separator → single string.',
    inputs: [{ id: 'then_in', name: 'Then', type: T.exec }, { id: 'arr', name: 'Array', type: T.array }],
    outputs: [{ id: 'then_out', name: 'Then', type: T.exec }, { id: 'out', name: 'Out', type: T.string }],
    params: [{ kind: 'string', key: 'sep', label: 'Separator', default: ' ', placeholder: '\\n' }],
  },
  {
    kind: 'string.replace',
    category: 'String',
    label: 'Replace',
    description: 'Literal find/replace on a single string or on every element of a string array. Global (replaces every occurrence, not just the first) and literal — meta-characters like `.` or `*` are not interpreted as regex. Empty `find` errors; an empty `replace` deletes every match. Output shape mirrors the input: scalar in → scalar out, array in → parallel array out, empty array in → empty array out. Non-string array elements (numbers, booleans, null) are coerced to their canonical JSON-to-string form before replacement, so a `null` slot emits an empty string (length preserved). The `In` and `Out` ports are typed `string-or-array` so the dual-shape contract is visible on the canvas. The `find` port is also `string-or-array`: a scalar string is a single pattern; an array is a *set* of patterns applied in sequence. The `replace` port is `string` only (scalar) — the single replacement string broadcasts across every pattern when `find` is an array (so `find=[a,b,c]` + `replace="X"` rewrites every occurrence of a, b, or c to X). Wiring an array into `replace` errors at runtime; if you need per-element replacements, route them through `string.concat` or `literal.json` upstream. The ports override `params.find` / `params.replace` when wired. Non-string port values are coerced to text before use (numbers become digits, booleans become `true`/`false`, `null` becomes empty); this matches the same coercion rule applied to the `in` port for non-string inputs.',
    inputs: [
      { id: 'then_in', name: 'Then', type: T.exec },
      { id: 'in', name: 'In', type: T['string-or-array'] },
      { id: 'find', name: 'Find', type: T['string-or-array'] },
      { id: 'replace', name: 'Replace', type: T.string },
    ],
    outputs: [{ id: 'then_out', name: 'Then', type: T.exec }, { id: 'out', name: 'Out', type: T['string-or-array'] }],
    params: [
      { kind: 'string', key: 'find', label: 'Find', default: '', placeholder: 'pattern' },
      { kind: 'string', key: 'replace', label: 'Replace', default: '', placeholder: 'replacement' },
    ],
  },
  {
    kind: 'array.textfilter',
    category: 'Array',
    label: 'Text Filter',
    description: 'Keep only elements of `in` whose relationship with `needle` matches the active `mode`. Six modes: `contains` / `startsWith` / `endsWith` (positive — keep when predicate is `true`) and `notContains` / `notStartsWith` / `notEndsWith` (negative — keep when the predicate is `false`). Defaults to `contains` when `mode` is unset. Case-sensitive — matches what you type. The `Needle` port AND the body `Needle (default)` text input both accept a SCALAR string OR an ARRAY of strings — multi-pattern shifts the predicate to EXISTENTIAL mode (`iter().any(|p| mode.matches(slot, p))`): a slot is kept iff AT LEAST ONE pattern satisfies the mode. For negative modes that means OR-of-`!matches` — equivalently drops only slots that match EVERY pattern (NOT all-match). The wired port wins over the body text input. Empty SCALAR wired (or `null`) collapses to the body text input (same empty-collapse-to-fallback contract as `image.format::resolve_format`); empty ARRAY wired DROPS every input slot — intentionally distinct from scalar-empty so wiring `array.create(count=0)` does NOT silently bypass your param fallback. Non-string wired values (numbers, booleans, null) coerce via canonical JSON text (Number → digits like `42` → `"42"`, Boolean → `"true"`/`"false"`, null → `""`); the text is compared against the active pattern set. Element-wise coercion applies to BOTH the `in` array slots AND the `needle` array slots; `null` needle slots are silently dropped after trim+filter so `[foo, "", null, bar]` collapses to `[foo, bar]` before the predicate runs. Scalar `Value::String` on `in` wraps as a 1-element array; `Value::Null` errors visibly. Toggle `Match All` (default off) to flip the multi-pattern predicate from `iter().any(...)` to `iter().all(...)` — slot kept iff EVERY pattern satisfies the mode under positive predicate, or iff EVERY pattern does NOT match under negative (drops only slots that satisfy every pattern). Empty pattern sets (wired empty array OR all-blank after trim+filter) drop every input slot REGARDLESS of `Match All` — explicit kill switch so an `array.create(count=0)` upstream cannot silently turn the filter into a no-op when the user toggles. Output is always `Value::Array`.',
    inputs: [
      { id: 'then_in', name: 'Then', type: T.exec },
      { id: 'in', name: 'In', type: T.array },
      { id: 'needle', name: 'Needle', type: T['string-or-array'] },
    ],
    outputs: [{ id: 'then_out', name: 'Then', type: T.exec }, { id: 'out', name: 'Out', type: T.array }],
    params: [
      { kind: 'select', key: 'mode', label: 'Mode', default: 'contains', options: ['contains', 'startsWith', 'endsWith', 'notContains', 'notStartsWith', 'notEndsWith'] },
      { kind: 'string', key: 'needle', label: 'Needle (default)', default: '', placeholder: 'search text' },
      { kind: 'boolean', key: 'matchAll', label: 'Match All', default: false },
    ],
  },

  // ---------- JSON ----------
  {
    kind: 'json.parse',
    category: 'Json',
    label: 'Parse',
    description: 'Parse a JSON string into a value. Errors on malformed input.',
    inputs: [{ id: 'then_in', name: 'Then', type: T.exec }, { id: 'text', name: 'Text', type: T.string }],
    outputs: [{ id: 'then_out', name: 'Then', type: T.exec }, { id: 'out', name: 'Out', type: T.any }],
    params: [],
  },
  {
    kind: 'json.stringify',
    category: 'Json',
    label: 'Stringify',
    description: 'Serialize a value as a JSON string. Toggle `pretty` for human-readable output.',
    inputs: [{ id: 'then_in', name: 'Then', type: T.exec }, { id: 'value', name: 'Value', type: T.any }],
    outputs: [{ id: 'then_out', name: 'Then', type: T.exec }, { id: 'out', name: 'Out', type: T.string }],
    params: [
      { kind: 'boolean', key: 'pretty', label: 'Pretty', default: false },
    ],
  },
  {
    kind: 'json.readFile',
    category: 'Json',
    label: 'Read File',
    description: 'Shortcut for read-then-parse: reads a JSON file at `path` and emits its value. Errors on I/O or parse failures.',
    inputs: [{ id: 'then_in', name: 'Then', type: T.exec }, { id: 'path', name: 'path', type: T.string }],

    outputs: [{ id: 'then_out', name: 'Then', type: T.exec }, { id: 'value', name: 'Value', type: T.any }],
    params: [
      { kind: 'string', key: 'path', label: 'Path', default: '', placeholder: '/path/to/data.json' },
    ],
  },

  // ---------- Array ----------
  {
    kind: 'array.length',
    category: 'Array',
    label: 'Length',
    description: 'Number of items in an array.',
    inputs: [{ id: 'then_in', name: 'Then', type: T.exec }, { id: 'in', name: 'In', type: T.array }],
    outputs: [{ id: 'then_out', name: 'Then', type: T.exec }, { id: 'out', name: 'Length', type: T.number }],
    params: [],
  },
  {
    kind: 'array.filter',
    category: 'Array',
    label: 'Filter ≥',
    description: 'Keep numeric items greater than or equal to threshold.',
    inputs: [{ id: 'then_in', name: 'Then', type: T.exec }, { id: 'in', name: 'In', type: T.array }],
    outputs: [{ id: 'then_out', name: 'Then', type: T.exec }, { id: 'out', name: 'Out', type: T.array }],
    params: [{ kind: 'number', key: 'threshold', label: 'Threshold', default: 0, step: 0.1 }],
  },
  {
    kind: 'array.sum',
    category: 'Array',
    label: 'Sum',
    description: 'Sum of all numeric items in an array.',
    inputs: [{ id: 'then_in', name: 'Then', type: T.exec }, { id: 'in', name: 'In', type: T.array }],
    outputs: [{ id: 'then_out', name: 'Then', type: T.exec }, { id: 'out', name: 'Sum', type: T.number }],
    params: [],
  },
  {
    kind: 'array.push',
    category: 'Array',
    label: 'Push',
    description: 'Append `item` to the end of `in`.',
    inputs: [{ id: 'then_in', name: 'Then', type: T.exec }, { id: 'in', name: 'In', type: T.array }, { id: 'item', name: 'Item', type: T.any }],
    outputs: [{ id: 'then_out', name: 'Then', type: T.exec }, { id: 'out', name: 'Out', type: T.array }],
    params: [],
  },
  {
    kind: 'array.create',
    category: 'Array',
    label: 'Create',
    description: 'Bundle N runtime inputs (field_1..field_N) into an array. Use the +/− buttons inside the node body or the Inspector `# Inputs` field to add or remove fields.',
    inputs: [{ id: 'then_in', name: 'Then', type: T.exec }],
    outputs: [{ id: 'then_out', name: 'Then', type: T.exec }, { id: 'out', name: 'Out', type: T.array }],
    params: [
      { kind: 'number', key: 'count', label: '# Inputs', default: 1, min: 1, max: 20 },
    ],
    repeatableInputs: {
      prefix: 'field',
      label: 'Field',
      type: T.any,
      min: 1,
      max: 20,
      countKey: 'count',
    },
  },
  {
    kind: 'array.getField',
    category: 'Array',
    label: 'Get Field',
    description: 'Extract one value from each item in `in`. `params.field` is interpreted by the element\'s runtime type — a property name for objects, or a numeric index (e.g. "2") for arrays. With `skipInvalid` enabled, non-object/array elements, missing keys, out-of-range indices, and non-numeric fields on array elements are silently dropped; otherwise they trigger a TypeMismatch mentioning the offending row.',
    inputs: [{ id: 'then_in', name: 'Then', type: T.exec }, { id: 'in', name: 'In', type: T.array }],
    outputs: [{ id: 'then_out', name: 'Then', type: T.exec }, { id: 'out', name: 'Out', type: T.array }],
    params: [
      { kind: 'string', key: 'field', label: 'Field', default: 'name', placeholder: 'field name or index' },
      { kind: 'boolean', key: 'skipInvalid', label: 'Skip invalid', default: false },
    ],
  },
  {
    kind: 'array.setField',
    category: 'Array',
    label: 'Set Field',
    description: 'Replace the value of `field` on every object in `in` with `value`. Missing fields are created (set semantics). With `skipInvalid` enabled, non-object elements are left untouched; otherwise they trigger a TypeMismatch.',
    inputs: [{ id: 'then_in', name: 'Then', type: T.exec }, { id: 'in', name: 'In', type: T.array }, { id: 'value', name: 'Value', type: T.any }],
    outputs: [{ id: 'then_out', name: 'Then', type: T.exec }, { id: 'out', name: 'Out', type: T.array }],
    params: [
      { kind: 'string', key: 'field', label: 'Field', default: 'name', placeholder: 'field name' },
      { kind: 'boolean', key: 'skipInvalid', label: 'Skip invalid', default: false },
    ],
  },

  // ---------- Inputs / Outputs ----------
  {
    kind: 'blueprint.input',
    category: 'IO',
    label: 'Blueprint Input',
    description: 'A blueprint-level input definition, shared across every instance in the graph (Blender group-input semantics). The output set is read from `metadata.inputs` on the parent blueprint — add or remove inputs via the Inspector\'s "Blueprint Inputs" section, and EVERY `Blueprint Input` node in the graph updates to surface the same ports. When you run the blueprint the Dashboard / Editor pre-Run modal asks for each declared input by `id` and the runner fills defaults from the blueprint so `blueprint.input::exec` only ever sees a fully-resolved map. Outputs intentionally read as `[]` here — the catalogue is informational; the canonical port schema lives on `metadata.inputs`.',
    inputs: [],
    outputs: [],
    params: [],
  },
  {
    kind: 'io.output',
    category: 'IO',
    label: 'Output',
    description: 'Exit point. Surfaces a value of the chosen type.',
    inputs: [{ id: 'then_in', name: 'Then', type: T.exec }, { id: 'in', name: 'In', type: T.any }],
    outputs: [],
    params: [{ kind: 'string', key: 'name', label: 'Name', default: 'output', placeholder: 'output_1' }],
  },
  {
    kind: 'io.logger',
    category: 'IO',
    label: 'Logger',
    description: 'Print any value with a label (no return).',
    inputs: [{ id: 'then_in', name: 'Then', type: T.exec }, { id: 'in', name: 'In', type: T.any }],
    outputs: [{ id: 'out', name: 'Pass', type: T.any }],
    params: [{ kind: 'string', key: 'label', label: 'Label', default: 'log', placeholder: 'label' }],
  },
  {
    kind: 'file.read',
    category: 'IO',
    label: 'Read',
    description: 'Read one or many files from disk. Single file → emits the full content as a string (`text`), a per-line array (`lines`), and base64-encoded raw bytes (`binary`) — connect the binary port to `image.format` or `file.saveBinary`. Array of paths → emits parallel arrays of `text` and `binary` (lines is omitted because there is no obvious shape for an array of per-line arrays). CRLF line endings are normalised to LF on the `lines` port; `text` keeps the original characters.',
    inputs: [{ id: 'then_in', name: 'Then', type: T.exec }],
    outputs: [
      { id: 'then_out', name: 'Then', type: T.exec },
      { id: 'text', name: 'Text', type: T['string-or-array'] },
      { id: 'lines', name: 'Lines', type: T.array },
      // TODO: switch to binary-or-array once introduced.
      { id: 'binary', name: 'Binary', type: T.binary },
    ],
    params: [
      { kind: 'string', key: 'path', label: 'Path', default: '', placeholder: '/path/to/file.txt' },
    ],
  },
  {
    kind: 'file.write',
    category: 'IO',
    label: 'Write',
    description: 'Write the connected `content` string to a file at the given `path`. Parent directories are created on demand. UTF-8.',
    inputs: [{ id: 'then_in', name: 'Then', type: T.exec }, { id: 'content', name: 'Content', type: T.string }],
    outputs: [],
    params: [
      { kind: 'string', key: 'path', label: 'Path', default: '', placeholder: '/path/to/output.txt' },
    ],
  },
  {
    kind: 'file.delete',
    category: 'IO',
    label: 'Delete',
    description: 'Remove one or more paths from disk. Default `recursive: false` is the safe mode — uses `fs::remove_file` and errors loudly on a directory path so a misclicked dropdown cannot `rm -rf` a folder. Toggle `recursive: true` to switch to `fs::remove_dir_all`, which works on files AND directories. Input `paths` is typed `string-or-array` so the dual-shape contract — scalar multi-line string OR array of strings — is visible on the canvas. When wired, the port overrides the body textarea; `file.iterate` is the typical upstream. Body textarea accepts one path per line; the executor splits on `\n` and trims blanks at run-time so a saved blueprint with `"paths": "a\\nb\\nc"` still deletes all three on next run. Empty lines and whitespace-only lines are silently dropped. The `deleted` output mirrors the effective input shape (single line → scalar out, multi-line or wired array → parallel array out, empty array → empty array out). Values echoed on `deleted` are trimmed (leading/trailing whitespace and blank lines dropped) when typed via the body textarea; values wired through the `paths` input port are echoed verbatim.',
    inputs: [{ id: 'then_in', name: 'Then', type: T.exec }, { id: 'paths', name: 'Paths', type: T['string-or-array'] }],
    outputs: [{ id: 'then_out', name: 'Then', type: T.exec }, { id: 'deleted', name: 'Deleted', type: T['string-or-array'] }],
    params: [
      {
        kind: 'string',
        key: 'paths',
        label: 'Paths (one per line)',
        default: '',
        placeholder: '/path/to/file\n/path/to/other',
      },
      { kind: 'boolean', key: 'recursive', label: 'Recursive', default: false },
    ],
  },

  {
    kind: "file.iterate",
    category: 'IO',
    label: 'Get Files in folder',
    description: 'Retrieve a list of files from a path',
    inputs: [{ id: 'then_in', name: 'Then', type: T.exec }, { id: 'path', name: 'Path', type: T.string }],
    outputs: [
      { id: 'then_out', name: 'Then', type: T.exec },
      { id: 'filenames', name: 'File names', type: T.array },
      { id: 'paths', name: 'Full paths', type: T.array },
    ],
    params: [
      { kind: 'string', key: 'path', label: 'Folder path', default: '', placeholder: '/path/to' },
    ],

  },

  // `file.pathPicker` — passthrough source node. The user's
  // selection(s) live in `params.path` (string for file/folder modes,
  // string[] for multiple-files). The catalog intentionally does NOT
  // expose `path` as an editable Inspector param — only `mode` —
  // because the value is set by the in-node Browse... button (via
  // `tauri-plugin-dialog`), not by typing. Wire the `path` output
  // into `file.read`'s `path` port, `file.saveBinary`'s `names`,
  // `image.format`'s `in`, etc. The downstream nodes already accept
  // either scalar or array shape, so the wireup is homogenous
  // regardless of which mode the picker was used in.
  {
    kind: 'file.pathPicker',
    category: 'IO',
    label: 'Path Picker',
    description: 'Open a system dialog to pick a folder or files at runtime. `mode`: "file" opens a single-file dialog (one string), "folder" opens a folder dialog (one string), "multiple-files" opens a multi-select file dialog (string array). Click Browse... in the node body to capture the selection. Output shape mirrors the mode (single=string, multi=string[]). Wire the `path` output into file.read, file.saveBinary, image.format, or any other node that already accepts string-or-array — downstream code already handles either shape uniformly. Note: changing the `mode` dropdown clears any previously selected paths — the shapes between modes (one path vs many) round-trip poorly in either direction, so a fresh pick keeps state sane.',
    inputs: [{ id: 'then_in', name: 'Then', type: T.exec }],
    outputs: [{ id: 'then_out', name: 'Then', type: T.exec }, { id: 'path', name: 'Path', type: T['string-or-array'] }],
    params: [
      { kind: 'select', key: 'mode', label: 'Mode', default: 'file', options: ['file', 'folder', 'multiple-files'] },
    ],
  },

  // ---------- Image / Binary pipeline ----------
  // The `binary` port type carries base64-encoded raw bytes; valid
  // pairings (single vs array) are handled per executor — see the
  // description on `file.saveBinary` for the full shape rules. The
  // source of binary data is `file.read` (its `binary` output port),
  // not a standalone `file.readBinary` node — we collapsed them.
  {
    kind: 'image.format',
    category: 'Image',
    label: 'Image Format',
    description: 'Re-encode one or many images into a different format (png/jpeg/webp/gif/bmp). Pixel data and resolution are unchanged. `quality` applies to JPEG only (image 0.25 WEBP encoder is lossless). Accepts single binary or an array of binaries; emits single or array to match the input shape. The wired `Format` input port takes precedence over the `Format (default)` body param when set to a non-empty trimmed string — wire a `literal.string`, route a value from `file.pathPicker`, or drive format dynamically from any other string source. Empty-string wired inputs collapse to the param fallback (intentional: lets you wire a placeholder literal without overwriting the param choice). When neither yields a usable value, defaults to `png`. Non-string wired values (numbers, booleans, null) are coerced to their canonical JSON text first; the result is then validated by `parse_format`, so a numeric literal wired here surfaces as `unsupported format "42"` instead of silently going to png.',
    // TODO: switch to binary-or-array once introduced (in/out).
    inputs: [{ id: 'then_in', name: 'Then', type: T.exec }, { id: 'format', name: 'Format', type: T.string }, { id: 'in', name: 'In', type: T.binary }],
    outputs: [{ id: 'then_out', name: 'Then', type: T.exec }, { id: 'out', name: 'Out', type: T.binary }],
    params: [
      { kind: 'select', key: 'format', label: 'Format (default)', default: 'png', options: ['png', 'jpeg', 'jpg', 'webp', 'gif', 'bmp'] },
      { kind: 'number', key: 'quality', label: 'Quality', default: 90, min: 1, max: 100, step: 1 },
    ],
  },
  {
    kind: 'file.saveBinary',
    category: 'IO',
    label: 'Save Binary',
    description: 'Write one or many base64-encoded payloads from `files` to the matching paths in `names`. Pair rules: Scalar-Scalar (1 file → 1 path), Scalar-Array (broadcast same file to N paths), Array-Array of equal length (N files → N paths). Array(files) + Scalar(names) errors to prevent overwriting. The `names` port is typed `string-or-array` and the `saved` output mirrors that — scalar in → scalar out, array in → parallel array out. (`files` stays typed `binary` because the binary-or-array sibling of this type lives in a future PR.)',
    inputs: [
      { id: 'then_in', name: 'Then', type: T.exec },
      // TODO: switch to binary-or-array once introduced.
      { id: 'files', name: 'Files', type: T.binary },
      { id: 'names', name: 'Names', type: T['string-or-array'] },
    ],
    outputs: [{ id: 'then_out', name: 'Then', type: T.exec }, { id: 'saved', name: 'Saved', type: T['string-or-array'] }],
    params: [],
  },
];

export const CATEGORIES: NodeCategory[] = [
  'Literal', 'Math', 'String', 'Json', 'Array', 'IO', 'Image',
];

export function getDef(kind: string): NodeDef | undefined {
  return NODE_CATALOG.find((d) => d.kind === kind);
}

/** Materialize defaults from the param definitions for a fresh node. */
export function defaultParams(def: NodeDef): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const p of def.params) {
    out[p.key] = p.default;
  }
  return out;
}

/**
 * Resolve the live count for a kind with `repeatableInputs`. Clamped to
 * the config's `min`/`max` so an out-of-range value can't crash the
 * renderer. Returns `null` if the kind does not declare `repeatableInputs`.
 */
export function resolvedRepeatCount(
  def: NodeDef | undefined,
  params: Record<string, unknown> | undefined,
): number | null {
  const cfg = def?.repeatableInputs;
  if (!cfg) return null;
  const key = cfg.countKey ?? 'count';
  const raw = Number(params?.[key] ?? cfg.min);
  if (!Number.isFinite(raw)) return cfg.min;
  return Math.max(cfg.min, Math.min(cfg.max, Math.trunc(raw)));
}

/**
 * Synthesise the input-port list for a kind that has `repeatableInputs`.
 * Pure helper used by both `composeBlueprint` (save) and `BlueprintNode`
 * (render).  Returns `null` for kinds without `repeatableInputs`.
 */
export function synthInputs(def: NodeDef, count: number): Port[] | null {
  const cfg = def.repeatableInputs;
  if (!cfg) return null;
  const n = Math.max(cfg.min, Math.min(cfg.max, count));
  return Array.from({ length: n }, (_, i) => ({
    id: `${cfg.prefix}_${i + 1}`,
    name: `${cfg.label} ${i + 1}`,
    type: cfg.type,
  }));
}

/** Convenience exported alias for `Port`. */
export type { Port };
