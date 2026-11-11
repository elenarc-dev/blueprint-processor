// Helpers for invoking the Rust `run_blueprint` command.
//
// The frontend composes the current Blueprint from runtime state (the
// writable Svelte Flow stores) plus an optional `inputs` map keyed by
// the `params.name` of any `io.input` nodes in the graph, then asks the
// Rust executor to evaluate it.

import { invoke } from '@tauri-apps/api/core';
import { composeBlueprint, type FlowNode, type FlowEdge } from './blueprint-io';
import type { BlueprintMeta } from '$lib/types/blueprint';

/** A value produced by `io.output`. */
export type RunOutput = unknown;

/** Severity of a log entry, mirroring `executor::LogLevel` in Rust. */
export type LogLevel = 'debug' | 'info' | 'warn' | 'error';

export interface RunLog {
  level: LogLevel;
  /** Node that emitted the entry; `''` for run-level entries. */
  node_id: string;
  /** `kind` of the emitting node (`'run'` for run-level entries). */
  kind: string;
  label: string;
  value: unknown;
}
export interface RunStats {
  nodes_executed: number;
  duration_ms: number;
}

/**
 * Result of the last run. A failed run is **not** an exception: it comes
 * back with `ok: false`, `error` set and the logs collected before the
 * failure, so the log panel can explain what went wrong.
 */
export interface RunResult {
  ok: boolean;
  outputs: Record<string, RunOutput>;
  logs: RunLog[];
  stats: RunStats;
  error: string | null;
}

/** Shape the JSON we pass to Rust: a complete Blueprint JSON document. */
function buildPayload(
  meta: BlueprintMeta,
  nodes: FlowNode[],
  edges: FlowEdge[],
  inputs: Record<string, unknown>,
): { json: string; parsedBlueprint: unknown } {
  const bp = composeBlueprint(meta, nodes, edges);
  return { json: JSON.stringify(bp), parsedBlueprint: bp };
}

/**
 * Run the current graph in the Rust executor. The graph itself is sent as
 * a json string (the same shape we serialise when saving); `inputs` is
 * the object mapping an `io.input` `params.name` → runtime value.
 *
 * Resolves for both outcomes of a run — inspect `ok`/`error`. Only a
 * transport-level failure (unknown command, unparseable blueprint) throws.
 */
export async function runBlueprint(
  meta: BlueprintMeta,
  nodes: FlowNode[],
  edges: FlowEdge[],
  inputs: Record<string, unknown> = {},
): Promise<RunResult> {
  const { json } = buildPayload(meta, nodes, edges, inputs);
  try {
    return await invoke<RunResult>('run_blueprint', { json, inputs });
  } catch (err) {
    // Rust ExecError serialises to its Display string; surface as an
    // exception-like object so the UI can render it.
    throw new Error(String(err));
  }
}

/** Serialize a single output for display in the results panel. */
export function renderValue(v: unknown): string {
  if (v === null || v === undefined) return 'null';
  if (typeof v === 'string') return v;
  if (typeof v === 'number' || typeof v === 'boolean') return String(v);
  if (Array.isArray(v)) {
    if (v.length > 24) {
      return `[${v.slice(0, 24).map(renderValue).join(', ')}, … (+${v.length - 24} more)]`;
    }
    return `[${v.map(renderValue).join(', ')}]`;
  }
  if (typeof v === 'object') return JSON.stringify(v);
  return String(v);
}
