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
  import { get, writable, type Writable } from 'svelte/store';
  import { page as pageStore } from '$app/stores';
  import { invoke } from '@tauri-apps/api/core';
  import { ask } from '@tauri-apps/plugin-dialog';
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
  import LogsPanel from '$lib/components/LogsPanel.svelte';
  import GraphBreadcrumb from '$lib/components/GraphBreadcrumb.svelte';
  import { blueprintToFlow, composeBlueprint } from '$lib/utils/blueprint-io';
  import { blueprintInputs } from '$lib/utils/blueprint-context.svelte';
  import {
    ensureSubgraph,
    invalidateSubgraph,
  } from '$lib/utils/subgraphs.svelte';
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

  /**
   * One ancestor kept while an embedded blueprint is being edited. The
   * frame owns its meta / nodes / edges so parent edits that were never
   * saved survive the round-trip into the child.
   */
  interface GraphFrame {
    name: string;
    path: string | null;
    meta: BlueprintMeta;
    nodes: FlowNode[];
    edges: FlowEdge[];
    /** Fingerprint of this level as of its last load / save. */
    baseline: string;
  }
  /** Ancestors of the graph on the canvas, outermost first. Empty at the
   *  root; every entry is a level the breadcrumb can return to. */
  let graphStack = $state<GraphFrame[]>([]);

  /**
   * Fingerprint of the live level as of its last load / save. Compared
   * against `fingerprint()` before navigating back so leaving unsaved
   * edits asks first. Empty means "not tracked yet" (no warning).
   */
  let savedBaseline = $state('');

  /** Breadcrumb entries: every ancestor plus the live graph. */
  const graphTrail = $derived([
    ...graphStack.map((frame, depth) => ({
      name: frame.name.trim() || frame.path || `Level ${depth + 1}`,
      depth,
    })),
    {
      name: bpMeta.name.trim() || currentFilePath || 'Untitled Blueprint',
      depth: graphStack.length,
    },
  ]);

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

  // Keep every `blueprint.subgraph` node's ports in sync with the file it
  // embeds. The node stores only a path; when that path has not been
  // resolved yet we load the blueprint once, replace the node's port set
  // and label, and drop any wire that pointed at a port which no longer
  // exists. `subgraphPath` records which path the current ports belong to
  // so the effect settles instead of reloading on every keystroke.
  $effect(() => {
    void syncSubgraphs(flowNodes);
  });

  async function syncSubgraphs(list: FlowNode[]) {
    for (const node of list) {
      const data = node.data as NodeData;
      if (data.kind !== 'blueprint.subgraph') continue;
      const path = String(data.params?.path ?? '').trim();
      if (!path || data.subgraphPath === path) continue;

      const def = await ensureSubgraph(path);
      nodes.update((arr: FlowNode[]) =>
        arr.map((n) =>
          n.id === node.id
            ? {
                ...n,
                data: {
                  ...n.data,
                  label: def.name || n.data.label,
                  inputs: def.inputs,
                  outputs: def.outputs,
                  subgraphPath: path,
                  params: { ...n.data.params, subgraphError: def.error },
                },
              }
            : n,
        ),
      );
      edges.update((prev: FlowEdge[]) =>
        prev.filter((e) => {
          if (e.source === node.id && !def.outputs.some((p) => p.id === e.sourceHandle)) {
            return false;
          }
          if (e.target === node.id && !def.inputs.some((p) => p.id === e.targetHandle)) {
            return false;
          }
          return true;
        }),
      );
    }
  }

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

    // "Open embedded blueprint" button in a subgraph / tool node. The
    // node is rendered by a generic component, so it talks to the page
    // through the same window-event pattern as palette click-to-spawn.
    const openHandler = (event: Event) => {
      const detail = (event as CustomEvent<{ path?: string }>).detail;
      if (detail?.path) void openSubgraph(detail.path);
    };
    window.addEventListener('blueprint:open-subgraph', openHandler as EventListener);

    // Handle dashboard deep-link `?path=…` (loaded by the Dashboard).
    const searchPath = $pageStore.url.searchParams.get('path');
    if (searchPath) {
      void loadFromPath(searchPath, /* stripQueryAfter */ true);
    }

    return () =>
      window.removeEventListener(
        'blueprint:open-subgraph',
        openHandler as EventListener,
      );
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
      await settleLevel();
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

  /** Clear run output when the canvas switches documents. */
  function resetRunState() {
    runResult = null;
    runError = null;
    logsOpen = false;
  }

  /**
   * Normalised serialisation of the live level for dirty checks. Mirrors
   * what Save writes (catalog defaults, regenerated ports) minus the
   * timestamp Save always bumps; transient Svelte Flow fields (selection,
   * measured size) never reach `composeBlueprint`.
   */
  function fingerprint(): string {
    const bp = composeBlueprint(bpMeta, get(nodes), get(edges));
    bp.updated_at = '';
    return JSON.stringify(bp);
  }

  /** The canvas now matches what is (or was last) on disk. */
  function markLevelSaved() {
    savedBaseline = fingerprint();
  }

  /**
   * Take the baseline after embedded subgraphs resolved: the sync effect
   * rewrites labels / ports from the files, and counting those system
   * writes as user edits would warn on an untouched level.
   */
  async function settleLevel() {
    await syncSubgraphs(get(nodes));
    savedBaseline = fingerprint();
  }

  /** Whether the live level differs from its last load / save. */
  function hasUnsavedChanges(): boolean {
    return savedBaseline !== '' && savedBaseline !== fingerprint();
  }

  // A fresh page starts with an empty canvas; capture its baseline so a
  // brand-new (never saved) level is not born dirty.
  savedBaseline = fingerprint();

  /**
   * Load `path` and edit it on the same canvas, keeping the current graph
   * underneath in `graphStack`. The live graph is snapshotted *before* the
   * child replaces it, so going back restores even unsaved parent edits.
   *
   * A path that is already in the chain is refused: opening it again would
   * either duplicate a level or create a cycle in the breadcrumb.
   */
  async function openSubgraph(rawPath: string) {
    const path = rawPath.trim();
    if (!path) {
      status = {
        message: 'Open subgraph: choose a blueprint for this node first',
        kind: 'error',
      };
      return;
    }
    if (running) {
      status = {
        message: 'Wait for the current run to finish before opening a subgraph',
        kind: 'error',
      };
      return;
    }
    if (path === currentFilePath || graphStack.some((frame) => frame.path === path)) {
      status = {
        message: `'${path}' is already open in this nesting chain`,
        kind: 'error',
      };
      return;
    }

    try {
      const json = await invoke<string>('load_blueprint', { path });
      const parsed = JSON.parse(json) as Blueprint;
      const flow = blueprintToFlow(parsed);
      const frame: GraphFrame = {
        name: bpMeta.name,
        path: currentFilePath,
        meta: {
          ...bpMeta,
          metadata: {
            ...bpMeta.metadata,
            inputs: bpMeta.metadata.inputs.map((input) => ({ ...input })),
          },
        },
        nodes: [...get(nodes)],
        edges: [...get(edges)],
        baseline: savedBaseline,
      };
      graphStack = [...graphStack, frame];
      nodes.set(flow.nodes);
      edges.set(flow.edges);
      bpMeta = stripMeta(parsed);
      currentFilePath = path;
      await settleLevel();
      resetRunState();
      status = { message: `Editing embedded blueprint → ${path}`, kind: 'info' };
    } catch (err) {
      status = { message: `Open subgraph failed: ${err}`, kind: 'error' };
    }
  }

  /**
   * Restore the frame at `depth` (an index in `graphStack`) and drop every
   * level below it. The child we are leaving is invalidated in the subgraph
   * cache and any restored node that embeds it has `subgraphPath` cleared,
   * so the sync effect re-derives ports from the (possibly edited) file.
   */
  async function navigateTo(depth: number) {
    if (depth < 0 || depth >= graphStack.length) return;
    if (hasUnsavedChanges()) {
      const name = bpMeta.name.trim() || currentFilePath || 'this graph';
      const discard = await ask(
        `Discard unsaved changes in "${name}" and go back?`,
        { title: 'Unsaved changes', kind: 'warning' },
      );
      if (!discard) return;
    }
    const leaving = [
      ...graphStack.slice(depth + 1).map((frame) => frame.path),
      currentFilePath,
    ].filter((path): path is string => !!path);
    const frame = graphStack[depth];
    for (const path of leaving) invalidateSubgraph(path);

    graphStack = graphStack.slice(0, depth);
    const restored = frame.nodes.map((node) => {
      const embedded = String(
        (node.data as NodeData | undefined)?.params?.path ?? '',
      );
      return embedded && leaving.includes(embedded)
        ? { ...node, data: { ...node.data, subgraphPath: undefined } }
        : node;
    });
    nodes.set(restored);
    edges.set(frame.edges);
    bpMeta = frame.meta;
    currentFilePath = frame.path;
    savedBaseline = frame.baseline;
    resetRunState();
    status = {
      message: `Back to ${frame.meta.name || frame.path || 'parent graph'}`,
      kind: 'info',
    };
  }

  function goBack() {
    void navigateTo(graphStack.length - 1);
  }

  /** Load / New replace the document, so the nesting chain no longer holds. */
  function resetGraphStack() {
    graphStack = [];
    savedBaseline = '';
    void settleLevel();
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
  let priorStorage = $state<{ key: string; bpId: string } | null>(null);
  $effect(() => {
    const next = inputsStorageKey;
    const bpId = bpMeta.id;
    const prev = priorStorage;
    // The write below schedules one extra Svelte 5 effect cycle —
    // on the next run, prev === next (settling), so the migration
    // guard correctly fails and the loop terminates. The double-run
    // is a documented Svelte 5 quirk, not a bug.
    priorStorage = { key: next, bpId };
    if (
      prev !== null &&
      prev.key !== next &&
      // Opening a subgraph also changes the key, but it changes the
      // blueprint too; migrating there would copy the parent's input
      // history into the child. Only a save of the SAME blueprint
      // (`bpMeta.id` unchanged) carries history to a path key.
      prev.bpId === bpId &&
      prev.key.startsWith('unsaved:') &&
      currentFilePath !== null
    ) {
      const carried = loadInputHistory(prev.key);
      if (carried && Object.keys(carried).length > 0) {
        saveInputHistory(next, carried);
      }
      // Clean up the unsaved slot regardless of whether anything
      // was carried: an empty prior unsaved entry leaves a stale
      // key in localStorage that a future `New` action could
      // collide with if the user picked up the same UUID (very
      // unlikely with crypto.randomUUID, but cheap to defensively
      // prune).
      clearInputHistory(prev.key);
    }
  });

  // Whether the pre-Run dialog is visible. Set by handleRun when we
  // detect any declared Blueprint Input; cleared by InputsDialog's
  // submit/cancel.
  let inputDialogOpen = $state(false);

  // Right-hand drawer with every log entry of the last run. Opens by
  // itself when a run fails, which is exactly when the logs matter.
  let logsOpen = $state(false);

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

      if (!result.ok) {
        runError = result.error ?? 'Run failed';
        logsOpen = true;
        status = { message: `Run failed: ${runError}`, kind: 'error' };
        return;
      }
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
    onResetNesting={resetGraphStack}
    onSaved={markLevelSaved}
    {logsOpen}
    logCount={runResult?.logs.length ?? 0}
    onToggleLogs={() => (logsOpen = !logsOpen)}
  />

  <GraphBreadcrumb trail={graphTrail} onNavigate={navigateTo} onBack={goBack} />

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
      onOpenSubgraph={openSubgraph}
    />
    <LogsPanel
      result={runResult}
      open={logsOpen}
      onClose={() => (logsOpen = false)}
    />
  </div>

  <ResultsPanel
    result={runResult}
    {running}
    onRun={handleRun}
    onClear={handleClearRun}
    onOpenLogs={() => (logsOpen = true)}
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
