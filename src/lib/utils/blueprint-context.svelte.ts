// Module-level reactive state for blueprint inputs.
//
// Why a module-level $state (instead of a Svelte context):
//   * Svelte 5 `.svelte.ts` files are rune modules — they support
//     `$state` at the top level, the same way a Svelte component would.
//   * A single module export is importable from both `+page.svelte`
//     (which pushes `bpMeta.metadata.inputs`) and `BlueprintNode.svelte`
//     (which renders output handles from it). Reads are reactive on
//     both ends automatically — no manual subscription plumbing.
//   * Exporting a tuple `{ value: ... }` rather than the bare array
//     keeps the object identity stable across reads; reassigning
//     `bpMeta.metadata.inputs` to a new array still triggers updates
//     because `$state` deeply-tracks the property.

import type { BlueprintInput } from '$lib/types/blueprint';

/**
 * Current blueprint inputs shared across the editor canvas. `+page.svelte`
 * mirrors `bpMeta.metadata.inputs` into `.value` whenever the user edits
 * them via the Inspector; every `BlueprintNode.svelte` instance reads it
 * to render its own output-handle set (so two `blueprint.input` nodes in
 * the same graph expose the SAME ports — Blender group-input semantics).
 *
 * **Set outside Svelte components only via `set(...)` below**, which
 * guarantees a fresh-array assignment (idempotent re-render trigger).
 */
export const blueprintInputs = $state<{ value: BlueprintInput[] }>({
  value: [],
});

/** Replace the shared value with a fresh array. Triggers reactivity. */
export function setBlueprintInputs(next: BlueprintInput[]) {
  blueprintInputs.value = next;
}

/**
 * Per-node map of wired input port ids. The catalog UI derives this
 * from the edges store so a node can ask "is THIS port wired?"
 * without subscribing to the full edges list (cheaper for graphs
 * with hundreds of nodes — BlueprintNode only re-runs the relevant
 * derived when its own set changes, not on every edge drag).
 *
 * **Mirroring rule:** every write to `edges` must also call
 * `recomputeWiredPortKeys($edges)` so the per-node projection stays
 * in sync. `Editor.svelte` does this in a `$effect` block. Reading
 * is reactive on the Svelte 5 rune system — BlueprintNode reads
 * `wiredPortKeys.value.get(id)` inside a `$derived` and observes
 * membership changes without manual subscription plumbing.
 */
export const wiredPortKeys = $state<{
  value: Map<string, Set<string>>;
}>({
  value: new Map(),
});

/**
 * Recompute the `nodeId → portIds` projection from the current
 * edges. Called from `Editor.svelte`'s `$effect`s — never from a
 * component render path (this would trigger every render to walk
 * the entire edge list, defeating the per-node reactivity we get
 * by mirroring once on edge change).
 */
export function recomputeWiredPortKeys(edges: Array<{ target?: string | null; targetHandle?: string | null }>) {
  const next = new Map<string, Set<string>>();
  for (const e of edges) {
    if (!e.target || !e.targetHandle) continue;
    let set = next.get(e.target);
    if (!set) {
      set = new Set<string>();
      next.set(e.target, set);
    }
    set.add(e.targetHandle);
  }
  wiredPortKeys.value = next;
}
