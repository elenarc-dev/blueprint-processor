// Library of embedded blueprints, keyed by file path.
//
// A `blueprint.subgraph` node only stores the path of the blueprint it
// embeds. The ports shown on the canvas belong to that file, not to the
// node, so the page resolves each path once and mirrors the result here.
// Nodes loaded from disk already carry their saved ports; this module is
// what keeps them in sync when the embedded file changed since.
//
// Failure is data, not an exception: a missing/invalid file yields a def
// with only the exec ports and an `error`, which the Inspector surfaces.

import { invoke } from '@tauri-apps/api/core';
import type { Blueprint, Port } from '$lib/types/blueprint';
import { portFromBlueprintInput } from './blueprint-io';

export interface SubgraphDef {
  path: string;
  name: string;
  inputs: Port[];
  outputs: Port[];
  error?: string;
}

export const subgraphDefs = $state<{ value: Record<string, SubgraphDef> }>({
  value: {},
});

const inFlight = new Map<string, Promise<SubgraphDef>>();

/** Ports for an embedded blueprint: declared inputs in, named outputs out. */
export function portsForBlueprint(bp: Blueprint): {
  inputs: Port[];
  outputs: Port[];
} {
  const inputs: Port[] = [{ id: 'then_in', name: 'Then', type: 'exec' }];
  for (const declared of bp.metadata.inputs) {
    const port = portFromBlueprintInput(declared);
    // `then_in` / `then_out` are reserved by the wrapper node.
    if (port.id === 'then_in' || port.id === 'then_out') continue;
    if (inputs.some((p) => p.id === port.id)) continue;
    inputs.push(port);
  }

  const outputs: Port[] = [{ id: 'then_out', name: 'Then', type: 'exec' }];
  for (const node of bp.nodes) {
    if (node.kind !== 'io.output') continue;
    const name = String((node.params as Record<string, unknown>)?.name ?? '')
      .trim() || 'output';
    if (name === 'then_in' || name === 'then_out') continue;
    if (outputs.some((p) => p.id === name)) continue;
    outputs.push({ id: name, name, type: 'any' });
  }

  return { inputs, outputs };
}

/** Exec-only ports, used when the embedded blueprint cannot be read. */
function fallbackDef(path: string, error: string): SubgraphDef {
  return {
    path,
    name: '',
    inputs: [{ id: 'then_in', name: 'Then', type: 'exec' }],
    outputs: [{ id: 'then_out', name: 'Then', type: 'exec' }],
    error,
  };
}

/**
 * Drop a cached definition so the next `ensureSubgraph` re-reads the file.
 *
 * Called when navigation returns from an embedded blueprint: its ports may
 * have changed while it was open, so the parent's subgraph node has to
 * re-derive them instead of trusting the cached shape.
 */
export function invalidateSubgraph(path: string): void {
  if (!(path in subgraphDefs.value)) return;
  const next = { ...subgraphDefs.value };
  delete next[path];
  subgraphDefs.value = next;
  inFlight.delete(path);
}

/**
 * Resolve a subgraph definition, caching both successes and failures.
 *
 * Concurrent callers for the same path share one IPC round-trip. A cached
 * failure is intentional: the page re-resolves only when the node's path
 * changes, so a broken path cannot spin the effect.
 */
export async function ensureSubgraph(path: string): Promise<SubgraphDef> {
  const cached = subgraphDefs.value[path];
  if (cached) return cached;
  const pending = inFlight.get(path);
  if (pending) return pending;

  const task = (async (): Promise<SubgraphDef> => {
    let def: SubgraphDef;
    try {
      const json = await invoke<string>('load_blueprint', { path });
      const bp = JSON.parse(json) as Blueprint;
      def = { path, name: bp.name, ...portsForBlueprint(bp) };
    } catch (err) {
      def = fallbackDef(path, String(err));
    }
    subgraphDefs.value = { ...subgraphDefs.value, [path]: def };
    inFlight.delete(path);
    return def;
  })();

  inFlight.set(path, task);
  return task;
}
