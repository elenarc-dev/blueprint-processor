<!--
  Blueprint Processor — main page (Editor route).

  State layout (no $effect cycles):
    bpMeta      $state · BlueprintMeta (id, name, description, metadata.inputs, …)
    nodes       writable<FlowNode[]>  · supplied to SvelteFlow
    edges       writable<FlowEdge[]>  · supplied to SvelteFlow
    flowNodes   $state mirror of `nodes` (for $derived)
    flowEdges   $state mirror of `edges` (for $derived)

  Cross-route plumbing:
    * `?path=<file>` on mount auto-loads via `load_blueprint` so the
      Dashboard's Open button deep-links into the editor.
    * `meta.metadata.inputs` is mirrored into the module-level
      `blueprintInputs` state so every Blueprint Input node in the
      graph surfaces the SAME output-port set (Blender group-input
      semantics).
    * Edges whose sourceHandle no longer corresponds to a declared
      meta.input (and whose source is a Blueprint Input node) are
      dropped on edit so the canvas never shows ghost connections.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { writable, type Writable } from 'svelte/store';
  import { page as pageStore } from '$app/stores';
  import { invoke } from '@tauri-apps/api/core';
  import { SvelteFlowProvider } from '@xyflow/svelte';

  import Toolbar from '$lib/components/Toolbar.svelte';
  import Palette from '$lib/components/Palette.svelte';
  import Editor from '$lib/components/Editor.svelte';
  import Inspector from '$lib/components/Inspector.svelte';
  import ResultsPanel from '$lib/components/ResultsPanel.svelte';
  import InputsDialog, {
    type InputRequest,
  } from '$lib/components/InputsDialog.svelte';
  import type { RunResult } from '$lib/utils/runner-io';
  import { runBlueprint as runInRust } from '$lib/utils/runner-io';
  import { blueprintToFlow } from '$lib/utils/blueprint-io';
  import { blueprintInputs } from '$lib/utils/blueprint-context.svelte';
  import {
    loadInputHistory,
    saveInputHistory,
    clearInputHistory,
  } from '$lib/utils/inputs-history';

  import {
    makeMeta,
    stripMeta,
    type BlueprintMeta,
    type Blueprint,
  } from '$lib/types/blueprint';
  import {
    type FlowNode,
    type FlowEdge,
    type NodeData,
  } from '$lib/utils/blueprint-io';

  const nodes: Writable<FlowNode[]> = writable<FlowNode[]>([]);
  const edges: Writable<FlowEdge[]> = writable<FlowEdge[]>([]);

  let bpMeta = $state<BlueprintMeta>(makeMeta('Untitled Blueprint'));
  let status = $state<{ message: string; kind: 'info' | 'error' }>({
    message: 'Ready',
    kind: 'info',
  });

  // Run state. `runResult` is the last successful RunResult; `runError`
  // is an error message if the run failed; `running` is a transient busy
  // flag (not synced to the Rust executor — Tauri returns a Future that
  // resolves when execution finishes).
  let runResult = $state<RunResult | null>(null);
  let runError = $state<string | null>(null);
  let running = $state(false);

  /**
   * Path of the file the current blueprint was loaded from, or the
   * path it was last saved to. The toolbar Save button saves in place
   * when this is non-null; otherwise it pops the OS picker. Cleared on
   * New and set by Load / Save / Save As… successful rounds and by
   * the dashboard's `?path=…` deep-link.
   */
  let currentFilePath = $state<string | null>(null);

  // Mirrors of the writables for reactive display.
  let flowNodes = $state<FlowNode[]>([]);
  let flowEdges = $state<FlowEdge[]>([]);

  $effect(() => nodes.subscribe((v) => (flowNodes = v)));
  $effect(() => edges.subscribe((v) => (flowEdges = v)));

  // Mirror `meta.metadata.inputs` into the shared module state so every
  // Blueprint Input node in the graph picks up the change without a
  // round-trip through `n.data.outputs`. The page is the only writer;
  // BlueprintNode is the only reader — they're decoupled by the module.
  $effect(() => {
    blueprintInputs.value = bpMeta.metadata.inputs;
  });

  // Drop orphan edges whose source is a Blueprint Input node and the
  // sourceHandle no longer matches any declared meta.input id. Runs on
  // every meta.inputs mutation. The page's `flowEdges` mirror is the
  // reactive source; we then push back into the writable.
  $effect(() => {
    const declaredIds = new Set(bpMeta.metadata.inputs.map((i) => i.id));
    // Snapshot the current state to avoid writing inside the same reactive pass.
    const current = flowEdges;
    const kept = current.filter((e) => {
      const srcNode = flowNodes.find((n) => n.id === e.source);
      const isBlueprintInput =
        (srcNode?.data as NodeData | undefined)?.kind === 'blueprint.input';
      if (!isBlueprintInput) return true;
      const handle = (e.sourceHandle ?? '') as string;
      return declaredIds.has(handle);
    });
    if (kept.length !== current.length) {
      edges.set(kept);
    }
  });

  onMount(() => {
    status = {
      message: 'Welcome · drag nodes from the left to begin',
      kind: 'info',
    };

    // Handle dashboard deep-link `?path=…` (loaded by the Dashboard).
    const searchPath = $pageStore.url.searchParams.get('path');
    if (searchPath) {
      void loadFromPath(searchPath, /* stripQueryAfter */ true);
    }
  });

  /** Load a blueprint file from disk and hydrate the editor state.
   *  `stripQueryAfter` removes the `?path=…` query param after the
   *  load so a manual reload doesn't re-trigger it. We strip on
   *  BOTH success and failure: a failed load that left the query
   *  param in place would loop on every browser refresh. */
  async function loadFromPath(filePath: string, stripQueryAfter = false) {
    try {
      const json = await invoke<string>('load_blueprint', { path: filePath });
      const parsed = JSON.parse(json) as Blueprint;
      const flow = blueprintToFlow(parsed);
      nodes.set(flow.nodes);
      edges.set(flow.edges);
      // `stripMeta` keeps every metadata field in sync rather than
      // rebuilding by hand — adding a new BlueprintMetadata field
      // later wouldn't silently drop it on load.
      bpMeta = stripMeta(parsed);
      currentFilePath = filePath;
      status = {
        message: `Loaded → ${filePath}`,
        kind: 'info',
      };
    } catch (err) {
      status = { message: `Load failed: ${err}`, kind: 'error' };
    } finally {
      if (stripQueryAfter) {
        try {
          const clean = new URL(window.location.href);
          clean.searchParams.delete('path');
          history.replaceState({}, '', clean.toString());
        } catch {
          // Browser-only API; SSR fallback or sandbox limitation
          // doesn't matter — editor runs Tauri-mode only.
        }
      }
    }
  }

  function commitMeta(next: BlueprintMeta) {
    bpMeta = next;
  }

  function commitPath(next: string | null) {
    currentFilePath = next;
  }

  // Apply a NodeData patch (label/params) coming from the Inspector.
  function handleNodeSave(nodeId: string, patch: Partial<NodeData>) {
    nodes.update((arr: FlowNode[]) =>
      arr.map((n) =>
        n.id === nodeId ? { ...n, data: { ...n.data, ...patch } } : n,
      ),
    );
  }

  // Pre-Run input requests: one per declared Blueprint Input. The
  // dialog's `InputRequest.type` is now `BlueprintInput['type']`
  // directly, so the picker kinds (`file`, `folder`, `multi-file`)
  // flow through to InputsDialog with no defensive coercion — the
  // dialog has an explicit `req.type` switch covering them (see
  // `$lib/components/InputsDialog.svelte`). Unknown values fall
  // through to the JSON textarea branch.
  const inputRequests = $derived<InputRequest[]>(
    bpMeta.metadata.inputs.map((inp, i) => ({
      name: inp.id,
      label: inp.name || inp.id,
      type: inp.type,
      nodeId: `meta:${i}:${inp.id}`,
    })),
  );

  const inputIds = $derived<string[]>(inputRequests.map((r) => r.name));

  // Per-blueprint key for the InputsDialog `storageKey` prop.
  //   * Saved blueprint → absolute on-disk path (stable across
  //     reloads of the same file).
  //   * Unsaved blueprint → `unsaved:<bpMeta.id>`. `makeMeta`
  //     calls crypto.randomUUID exactly once per `bpMeta`, so two
  //     unsaved sessions on the same form never share a key by
  //     accident; the `unsaved:` prefix distinguishes transient
  //     session ids from real paths so the migration in the
  //     effect below can pivot cleanly when the user first Saves.
  // `bpMeta.id` is unconditional (always set by makeMeta on mount),
  // so the alternative would just be dead-branching on a string
  // that's always present; `string` is the precise type.
  const inputsStorageKey = $derived<string>(
    currentFilePath ?? `unsaved:${bpMeta.id}`,
  );

  // Storage-key-migration: when the user first saves a blueprint
  // they had been running as `unsaved:<uuid>`, the prior inputs
  // they typed would otherwise be orphaned under the unsaved key
  // and silently lost on the next re-run of the saved file. This
  // effect watches the inputsStorageKey derivation and, on the
  // transition from an unsaved UUID to a real on-disk path,
  // copies the prior unsaved entry over and clears it. Tracks the
  // prior key in a `$state` mirror so the effect can detect the
  // monotonic transition (read-once via `untrack` so the effect
  // doesn't loop on its own write).
  let priorStorageKey = $state<string | null>(null);
  $effect(() => {
    const next = inputsStorageKey;
    const prev = priorStorageKey;
    // The write below schedules one extra Svelte 5 effect cycle —
    // on the next run, prev === next (settling), so the migration
    // guard correctly fails and the loop terminates. The double-run
    // is a documented Svelte 5 quirk, not a bug.
    priorStorageKey = next;
    if (
      prev !== null &&
      prev !== next &&
      prev.startsWith('unsaved:') &&
      currentFilePath !== null
    ) {
      const carried = loadInputHistory(prev);
      if (carried && Object.keys(carried).length > 0) {
        saveInputHistory(next, carried);
      }
      // Clean up the unsaved slot regardless of whether anything
      // was carried: an empty prior unsaved entry leaves a stale
      // key in localStorage that a future `New` action could
      // collide with if the user picked up the same UUID (very
      // unlikely with crypto.randomUUID, but cheap to defensively
      // prune).
      clearInputHistory(prev);
    }
  });

  // Whether the pre-Run dialog is visible. Set by handleRun when we
  // detect any declared Blueprint Input; cleared by InputsDialog's
  // submit/cancel.
  let inputDialogOpen = $state(false);

  async function handleRun() {
    if (running) return;
    // Gate: if the graph declares any Blueprint Input, open the
    // dialog first so the user can supply values before we kick
    // off the run.
    if (inputRequests.length > 0) {
      inputDialogOpen = true;
      return;
    }
    await executeRun({});
  }

  async function executeRun(inputs: Record<string, unknown>) {
    running = true;
    runError = null;
    try {
      const result = await runInRust(bpMeta, flowNodes, flowEdges, inputs);
      runResult = result;
      const last =
        result.logs.length > 0
          ? result.logs[result.logs.length - 1].label
          : `${Object.keys(result.outputs).length} outputs`;
      status = {
        message: `Run ok · ${result.stats.nodes_executed} nodes · ${result.stats.duration_ms} ms · last: ${last}`,
        kind: 'info',
      };
    } catch (err) {
      runError = String(err);
      runResult = null;
      status = { message: `Run failed: ${err}`, kind: 'error' };
    } finally {
      running = false;
    }
  }

  function handleClearRun() {
    runResult = null;
    runError = null;
  }

  // Surface the inventory upsert side-effect: status message posted
  // from Toolbar after a successful Save reports whether the file
  // landed on the dashboard; we use this for the explicit warning if
  // it didn't.
  function onAfterIoWithInventory(message: string, kind: 'info' | 'error') {
    status = { message, kind };
  }
</script>

<div class="app-shell">
  <Toolbar
    meta={bpMeta}
    {nodes}
    {edges}
    currentFilePath={currentFilePath}
    onMetaChange={commitMeta}
    onPathChange={commitPath}
    onAfterIo={onAfterIoWithInventory}
  />

  <div class="main">
    <Palette />
    <div class="canvas-wrap" data-testarea="canvas">
      <SvelteFlowProvider>
        <Editor {nodes} {edges} />
      </SvelteFlowProvider>
      {#if flowNodes.length === 0}
        <div class="empty-canvas">
          <h2>Empty blueprint</h2>
          <p>
            Drag a node from the left palette to get started. Connect
            handles by dragging between them.
          </p>
        </div>
      {/if}
    </div>
    <Inspector
      meta={bpMeta}
      nodes={flowNodes}
      onMetaChange={commitMeta}
      onNodeSave={handleNodeSave}
    />
  </div>

  <ResultsPanel
    result={runResult}
    {running}
    onRun={handleRun}
    onClear={handleClearRun}
  />

  {#if inputDialogOpen}
    <InputsDialog
      inputs={inputRequests}
      storageKey={inputsStorageKey}
      onSubmit={(values) => {
        inputDialogOpen = false;
        void executeRun(values);
      }}
      onCancel={() => (inputDialogOpen = false)}
    />
  {/if}

  <div class="status-bar">
    <span class="badge">v{bpMeta.schema_version}</span>
    <span class={status.kind === 'error' ? 'err' : 'ok'}>●</span>
    <span>{status.message}</span>
    <span class="badge">{flowNodes.length} nodes</span>
    <span class="badge">{flowEdges.length} edges</span>
    {#if inputIds.length > 0}
      <span class="badge">inputs: {inputIds.join(', ')}</span>
    {/if}
  </div>
</div>

<style>
  .app-shell {
    display: flex;
    flex-direction: column;
    height: 100vh;
    width: 100vw;
    background: #f4f4f5;
  }
  .main {
    flex: 1;
    display: flex;
    min-height: 0;
  }
  .canvas-wrap {
    flex: 1;
    position: relative;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }

  /* Help message that overlays the canvas when empty. Must be
     pointer-events: none so HTML5 drag-and-drop events pass through to
     the .canvas-host drop target below. */
  .empty-canvas {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
    pointer-events: none;
    background: linear-gradient(
      180deg,
      rgba(244, 244, 245, 0) 0%,
      rgba(244, 244, 245, 0.55) 60%,
      rgba(244, 244, 245, 0.7) 100%
    );
    color: #52525b;
    text-align: center;
    padding: 24px;
  }
  .empty-canvas h2 {
    margin: 0;
    font-size: 16px;
    font-weight: 600;
    letter-spacing: 0.02em;
    color: #27272a;
  }
  .empty-canvas p {
    margin: 0;
    font-size: 12.5px;
    max-width: 320px;
    line-height: 1.5;
  }
</style>
