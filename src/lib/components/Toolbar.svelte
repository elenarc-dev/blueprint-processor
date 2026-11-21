<!--
  Top toolbar. Save / Load / New plus blueprint name editing.

  Snapshots for Save are obtained via `get(store)` from svelte/store so
  we read the current value without taking a subscription.
-->
<script lang="ts">
  import { get, type Writable } from 'svelte/store';
  import { invoke } from '@tauri-apps/api/core';
  import {
    save as saveDialog,
    open as openDialog,
    ask,
  } from '@tauri-apps/plugin-dialog';
  import type { Blueprint, BlueprintInput, Port } from '$lib/types/blueprint';
  import { makeMeta, type BlueprintMeta } from '$lib/types/blueprint';
  import AiProvidersDialog from './AiProvidersDialog.svelte';
  import AiToolsDialog from './AiToolsDialog.svelte';
  import {
    blueprintToFlow,
    composeBlueprint,
    makeFlowNode,
    newEdgeId,
    portFromBlueprintInput,
    type FlowNode,
    type FlowEdge,
  } from '$lib/utils/blueprint-io';
  import { portsForBlueprint } from '$lib/utils/subgraphs.svelte';

  interface Props {
    meta: BlueprintMeta;
    nodes: Writable<FlowNode[]>;
    edges: Writable<FlowEdge[]>;
    /**
     * Path the current blueprint was loaded from, or last saved to.
     * When non-null the Save button bypasses the file picker and
     * rewrites that path in place. Cleared on New.
     */
    currentFilePath: string | null;
    onMetaChange: (next: BlueprintMeta) => void;
    /**
     * Toolbar pushes the canonical resolved path back to the page
     * after every successful Load / Save / Save As… so the future
     * Save is in-place without prompting again.
     */
    onPathChange: (next: string | null) => void;
    onAfterIo: (msg: string, kind: 'info' | 'error') => void;
    /**
     * Load / New replace the whole document, so any subgraph nesting chain
     * the page is holding no longer applies. Called on the success path of
     * those two actions.
     */
    onResetNesting: () => void;
    /**
     * Called after a successful Save / Save As… so the page can re-arm the
     * unsaved-changes baseline of the level on the canvas.
     */
    onSaved: () => void;
    /** Whether the right-hand run-log drawer is open. */
    logsOpen: boolean;
    /** Entry count of the last run, shown next to the Logs button. */
    logCount: number;
    onToggleLogs: () => void;
  }
  let {
    meta,
    nodes,
    edges,
    currentFilePath,
    onMetaChange,
    onPathChange,
    onAfterIo,
    onResetNesting,
    onSaved,
    logsOpen,
    logCount,
    onToggleLogs,
  }: Props = $props();

  let saving = $state(false);
  let loading = $state(false);
  /** Nodes selected on the canvas — enables "Group as…". */
  let selectionCount = $state(0);
  $effect(() =>
    nodes.subscribe(
      (list) =>
        (selectionCount = list.filter((node) => node.selected).length),
    ),
  );
  /**
   * One-step undo of the last **Group as…**: the level as it was before the
   * swap. Cloned on capture so later drag/selection mutations cannot leak in.
   */
  let groupUndo = $state<{ nodes: FlowNode[]; edges: FlowEdge[] } | null>(null);
  /** Global AI providers (endpoints, models, keys) — see AiProvidersDialog. */
  let providersOpen = $state(false);
  /** Global app-level AI tools — see AiToolsDialog. */
  let toolsOpen = $state(false);

  async function handleSave() {
    try {
      saving = true;
      // If a file was loaded (or the previous Save/Save As… resolved a
      // path), reuse it directly. The `save_blueprint` Tauri command
      // accepts a path arg, so the dialog is bypassed and the existing
      // file is overwritten atomically (see `write_atomically` in
      // src-tauri/src/lib.rs).
      let path = currentFilePath;
      if (!path) {
        path = await saveDialog({
          title: 'Save blueprint',
          defaultPath: `${meta.name || 'untitled'}.blueprint.json`,
          filters: [
            { name: 'Blueprint JSON', extensions: ['blueprint.json', 'json'] },
          ],
        });
        if (!path) return;
      }
      const payload: Blueprint = composeBlueprint(
        meta,
        get(nodes),
        get(edges),
      );
      const json = JSON.stringify(payload, null, 2);
      const result = await invoke<{ path: string; bytes: number }>(
        'save_blueprint',
        { json, path },
      );
      onPathChange(result.path);
      onSaved();
      onAfterIo(`Saved ${result.bytes} B → ${result.path}`, 'info');
    } catch (err) {
      onAfterIo(`Save failed: ${err}`, 'error');
    } finally {
      saving = false;
    }
  }

  async function handleSaveAs() {
    try {
      saving = true;
      // Always opens the picker regardless of currentFilePath — the
      // whole point of "Save As…" is to retarget the file.
      const path = await saveDialog({
        title: 'Save blueprint as…',
        defaultPath: `${meta.name || 'untitled'}.blueprint.json`,
        filters: [
          { name: 'Blueprint JSON', extensions: ['blueprint.json', 'json'] },
        ],
      });
      if (!path) return;
      const payload: Blueprint = composeBlueprint(
        meta,
        get(nodes),
        get(edges),
      );
      const json = JSON.stringify(payload, null, 2);
      const result = await invoke<{ path: string; bytes: number }>(
        'save_blueprint',
        { json, path },
      );
      onPathChange(result.path);
      onSaved();
      onAfterIo(`Saved ${result.bytes} B → ${result.path}`, 'info');
    } catch (err) {
      onAfterIo(`Save As failed: ${err}`, 'error');
    } finally {
      saving = false;
    }
  }

  /**
   * Save the selected nodes as their own blueprint and replace them in this
   * graph with one `blueprint.subgraph` node pointing at the new file.
   *
   * Wiring: wires with both ends inside the selection travel with the group.
   * Data wires arriving from outside become declared inputs of the group —
   * each id derived from the sending node and the port, so two consumers of
   * the same port name stay distinct — with a `blueprint.input` node inside
   * feeding the same internal port, so they are reattached to the subgraph
   * node on the way in. Wires leaving the
   * selection reconnect when their source port is one the group exposes: the
   * exec `then_out` and every `io.output` name. What has no counterpart on
   * the wrapper is dropped.
   */
  async function handleSaveSelection() {
    const allNodes = get(nodes);
    const selected = allNodes.filter((node) => node.selected);
    if (selected.length === 0) {
      onAfterIo('Select one or more nodes to group', 'error');
      return;
    }
    const selectedIds = new Set(selected.map((node) => node.id));
    const innerEdges = get(edges).filter(
      (edge) => selectedIds.has(edge.source) && selectedIds.has(edge.target),
    );
    const outerEdges = get(edges).filter(
      (edge) => selectedIds.has(edge.source) !== selectedIds.has(edge.target),
    );

    const name = `${meta.name.trim() || 'Untitled'} · group`;
    const subMeta = makeMeta(name);
    subMeta.description = `Group of ${selected.length} nodes extracted from ${
      meta.name.trim() || 'an untitled blueprint'
    }.`;

    // Declare the group's data inputs from the wires arriving from outside.
    // Ids are derived from the sending node plus the port, so two consumers of
    // the same port name stay distinct instead of collapsing. A declaration
    // only opens the group's interface, so one `blueprint.input` node is
    // injected and wired to the same target port: at run time the subgraph
    // executor forwards incoming values as `external_inputs` keyed by id.
    const incoming = outerEdges.filter((edge) => selectedIds.has(edge.target));
    const byId = new Map(allNodes.map((node) => [node.id, node]));
    const declared: BlueprintInput[] = [];
    /** Outer edge id → id of the group input it feeds. */
    const declaredIdByEdge = new Map<string, string>();
    const usedIds = new Set<string>();
    const usedNames = new Set<string>();
    for (const edge of incoming) {
      const port = byId
        .get(edge.target)
        ?.data.inputs.find((p) => p.id === String(edge.targetHandle ?? ''));
      if (!port || port.type === 'exec') continue;
      const source = byId.get(edge.source);
      const sourceLabel = source?.data.label || source?.data.kind || 'input';
      const base = tokenId(`${sourceLabel}_${port.id}`);
      let id = base;
      for (let n = 2; usedIds.has(id); n++) id = `${base}_${n}`;
      usedIds.add(id);
      declaredIdByEdge.set(edge.id, id);
      // Repeated names would make the run modal ambiguous; the first keeps the
      // bare label, later ones say where they come from.
      const name = usedNames.has(port.name)
        ? `${sourceLabel} · ${port.name}`
        : port.name;
      usedNames.add(name);
      declared.push({ id, name, type: inputTypeForPort(port.type) });
    }

    const groupedNodes = [...selected];
    const groupedEdges = [...innerEdges];
    if (declared.length > 0) {
      subMeta.metadata.inputs = declared;
      const inputNode = makeFlowNode('blueprint.input', {
        x: Math.min(...selected.map((node) => node.position.x)) - 260,
        y: Math.min(...selected.map((node) => node.position.y)),
      });
      inputNode.data.outputs = declared.map(portFromBlueprintInput);
      groupedNodes.push(inputNode);
      // One feeder per declared input, back to the internal port that used to
      // receive the value.
      for (const edge of incoming) {
        const id = declaredIdByEdge.get(edge.id);
        if (!id) continue;
        groupedEdges.push({
          id: newEdgeId(inputNode.id, edge.target),
          source: inputNode.id,
          sourceHandle: id,
          target: edge.target,
          targetHandle: String(edge.targetHandle ?? ''),
        });
      }
    }
    const grouped = composeBlueprint(subMeta, groupedNodes, groupedEdges);

    let path: string | string[] | null = null;
    try {
      path = await saveDialog({
        title: 'Save selected nodes as a blueprint',
        defaultPath: `${slugify(name)}.blueprint.json`,
        filters: [
          { name: 'Blueprint JSON', extensions: ['blueprint.json', 'json'] },
        ],
      });
    } catch (err) {
      onAfterIo(`Group failed: ${err}`, 'error');
      return;
    }
    if (!path || Array.isArray(path)) return;

    try {
      saving = true;
      const result = await invoke<{ path: string; bytes: number }>(
        'save_blueprint',
        { json: JSON.stringify(grouped, null, 2), path },
      );

      // Capture before the swap; the grouped file stays on disk either way.
      groupUndo = {
        nodes: get(nodes).map((node) => ({ ...node, data: { ...node.data } })),
        edges: get(edges).map((edge) => ({ ...edge })),
      };

      const center = selected.reduce(
        (acc, node) => ({
          x: acc.x + node.position.x / selected.length,
          y: acc.y + node.position.y / selected.length,
        }),
        { x: 0, y: 0 },
      );
      const ports = portsForBlueprint(grouped);
      const subgraph = makeFlowNode('blueprint.subgraph', center);
      subgraph.data.params = { ...subgraph.data.params, path: result.path };
      subgraph.data.label = name;
      // Mirror the file's ports now: the reconnection below runs against the
      // real contract, and `subgraphPath` tells the page's sync these ports
      // already belong to that file (no extra read, no placeholder flash).
      subgraph.data.inputs = ports.inputs;
      subgraph.data.outputs = ports.outputs;
      subgraph.data.subgraphPath = result.path;

      const inputIds = new Set(ports.inputs.map((port) => port.id));
      const outputIds = new Set(ports.outputs.map((port) => port.id));
      const takenInputs = new Set<string>();
      const takenOutputs = new Set<string>();
      const rewired: FlowEdge[] = [];
      let dropped = 0;
      for (const edge of outerEdges) {
        const sourceHandle = String(edge.sourceHandle ?? '');
        const targetHandle = String(edge.targetHandle ?? '');
        if (selectedIds.has(edge.target)) {
          // Outside → group: the declared id when the wire became an input,
          // else a port the group already exposes (`then_in`). One wire per
          // input, matching the canvas rule.
          const handle = declaredIdByEdge.get(edge.id) ?? targetHandle;
          if (!inputIds.has(handle) || takenInputs.has(handle)) {
            dropped++;
            continue;
          }
          takenInputs.add(handle);
          rewired.push({
            id: newEdgeId(edge.source, subgraph.id),
            source: edge.source,
            sourceHandle,
            target: subgraph.id,
            targetHandle: handle,
          });
        } else {
          // Group → outside. An output may fan out, so only identical wires
          // collapse.
          const key = `${sourceHandle}\u0000${edge.target}\u0000${targetHandle}`;
          if (!outputIds.has(sourceHandle) || takenOutputs.has(key)) {
            dropped++;
            continue;
          }
          takenOutputs.add(key);
          rewired.push({
            id: newEdgeId(subgraph.id, edge.target),
            source: subgraph.id,
            sourceHandle,
            target: edge.target,
            targetHandle,
          });
        }
      }

      nodes.set([
        ...get(nodes).filter((node) => !selectedIds.has(node.id)),
        subgraph,
      ]);
      edges.set([
        ...get(edges).filter(
          (edge) =>
            !selectedIds.has(edge.source) && !selectedIds.has(edge.target),
        ),
        ...rewired,
      ]);

      onAfterIo(
        `Grouped ${selected.length} nodes → ${result.path}` +
          (rewired.length > 0
            ? ` · ${rewired.length} outer wire(s) reconnected`
            : '') +
          (dropped > 0 ? ` · ${dropped} outer wire(s) dropped` : ''),
        'info',
      );
    } catch (err) {
      onAfterIo(`Group failed: ${err}`, 'error');
    } finally {
      saving = false;
    }
  }

  /** Reverse of `portTypeFromInputType`, for declaring group inputs. */
  function inputTypeForPort(type: Port['type']): BlueprintInput['type'] {
    switch (type) {
      case 'string':
      case 'number':
      case 'boolean':
      case 'array':
        return type;
      default:
        return 'any';
    }
  }

  /** Restore the graph replaced by the last **Group as…**. */
  function handleUndoGroup() {
    if (!groupUndo) return;
    nodes.set(groupUndo.nodes);
    edges.set(groupUndo.edges);
    groupUndo = null;
    onAfterIo('Group undone — original nodes and wires restored', 'info');
  }

  /** Token-shaped id for a declared group input (`metadata.inputs` key). */
  function tokenId(value: string): string {
    const id = value
      .normalize('NFD')
      .replace(/[\u0300-\u036f]/g, '')
      .replace(/[^a-zA-Z0-9]+/g, '_')
      .replace(/^_+|_+$/g, '');
    return id || 'input';
  }

  /** Filename-safe slug for the default path of a grouped blueprint. */
  function slugify(value: string): string {
    const slug = value
      .normalize('NFD')
      .replace(/[\u0300-\u036f]/g, '')
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, '-')
      .replace(/^-+|-+$/g, '');
    return slug || 'subgraph';
  }

  async function handleLoad() {
    try {
      loading = true;
      const path = await openDialog({
        title: 'Load blueprint',
        multiple: false,
        filters: [
          { name: 'Blueprint JSON', extensions: ['blueprint.json', 'json'] },
        ],
      });
      if (!path || Array.isArray(path)) return;
      const json = await invoke<string>('load_blueprint', { path });
      const parsed = JSON.parse(json) as Blueprint;
      const flow = blueprintToFlow(parsed);
      nodes.set(flow.nodes);
      edges.set(flow.edges);
      onMetaChange({
        id: parsed.id,
        name: parsed.name,
        description: parsed.description,
        created_at: parsed.created_at,
        updated_at: parsed.updated_at,
        schema_version: parsed.schema_version,
        metadata: parsed.metadata,
      });
      // Track the loaded path so subsequent Save preserves it
      // (works even if the user never explicitly saves — "Load" is
      // an implicit bookmark that the next Save in-place honors).
      onPathChange(path);
      onResetNesting();
      groupUndo = null;
      onAfterIo(`Loaded → ${path}`, 'info');
    } catch (err) {
      onAfterIo(`Load failed: ${err}`, 'error');
    } finally {
      loading = false;
    }
  }

  async function handleNew() {
    if (get(nodes).length > 0) {
      const yes = await ask(
        'Start a new blueprint? Unsaved changes will be lost.',
        { title: 'New blueprint', kind: 'warning' },
      );
      if (!yes) return;
    }
    nodes.set([]);
    edges.set([]);
    // Fresh metadata envelope — `makeMeta` produces a new UUID + stamps
    // creation time. Avoids stale `metadata.tags` / `id` leaking in via
    // an object spread.
    onMetaChange(makeMeta('Untitled Blueprint'));
    // Forget the previous file — the next Save should re-open the
    // picker instead of silently overwriting the old file.
    onPathChange(null);
    onResetNesting();
    groupUndo = null;
    onAfterIo('New blueprint ready', 'info');
  }

  function handleName(e: Event) {
    onMetaChange({
      ...meta,
      name: (e.target as HTMLInputElement).value,
      updated_at: new Date().toISOString(),
    });
  }
</script>

<header class="toolbar">
  <div class="brand">
    <span class="logo">◇</span>
    <span class="title">Blueprint Processor</span>
  </div>

  <div class="separator"></div>

  <button class="btn" onclick={handleNew}>New</button>
  <button class="btn" onclick={handleLoad} disabled={loading}>
    {loading ? 'Loading…' : 'Load'}
  </button>
  <button class="btn btn-primary" onclick={handleSave} disabled={saving}>
    {saving ? 'Saving…' : 'Save'}
  </button>
  <button
    class="btn"
    onclick={handleSaveAs}
    disabled={saving}
    title="Save to a new file…"
  >
    Save As…
  </button>
  <button
    class="btn"
    onclick={handleSaveSelection}
    disabled={saving || selectionCount === 0}
    title={selectionCount === 0
      ? 'Select nodes on the canvas first'
      : `Save ${selectionCount} selected node${selectionCount === 1 ? '' : 's'} as a blueprint and replace them with a Subgraph node`}
  >
    Group as…
  </button>
  {#if groupUndo}
    <button
      class="btn"
      onclick={handleUndoGroup}
      disabled={saving}
      title="Restore the nodes and wires replaced by the last Group as…"
    >
      Undo group
    </button>
  {/if}

  <div class="separator"></div>

  <button
    class="btn"
    onclick={() => (providersOpen = true)}
    title="Configure AI providers, models and API keys"
  >
    AI Providers
  </button>
  <button
    class="btn"
    onclick={() => (toolsOpen = true)}
    title="Define HTTP tools that AI nodes can call"
  >
    AI Tools
  </button>
  <button
    class="btn"
    onclick={onToggleLogs}
    title="Show every log entry of the last run"
  >
    Logs{logCount > 0 ? ` · ${logCount}` : ''}
  </button>

  <div class="separator"></div>

  <label class="name-field">
    <span class="muted">Name</span>
    <input
      type="text"
      class="name-input"
      value={meta.name}
      oninput={handleName}
      placeholder="Blueprint name"
    />
  </label>

  <div class="spacer"></div>

  <div class="hint">Drag nodes from the left panel</div>
</header>

<AiProvidersDialog bind:open={providersOpen} />
<AiToolsDialog bind:open={toolsOpen} />

<style>
  .toolbar {
    height: 46px;
    flex-shrink: 0;
    background: linear-gradient(to bottom, #fafafa, #f0f0f3);
    border-bottom: 1px solid #d4d4d8;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 12px;
    font-family: 'Inter', system-ui, sans-serif;
    box-shadow: 0 1px 0 rgba(0, 0, 0, 0.02);
  }

  .brand {
    display: flex;
    align-items: center;
    gap: 8px;
    color: #27272a;
    font-weight: 700;
  }
  .logo {
    color: #2d7cf1;
    font-size: 18px;
  }
  .title {
    font-size: 13.5px;
    letter-spacing: 0.01em;
  }

  .separator {
    width: 1px;
    height: 22px;
    background: #d4d4d8;
    margin: 0 4px;
  }

  .btn {
    height: 30px;
    padding: 0 12px;
    background: white;
    border: 1px solid #d4d4d8;
    border-radius: 5px;
    color: #27272a;
    font-size: 12.5px;
    font-weight: 500;
    cursor: pointer;
    transition:
      background 0.1s,
      border-color 0.1s,
      transform 0.05s;
  }
  .btn:hover:not(:disabled) {
    background: #f4f4f5;
    border-color: #a1a1aa;
  }
  .btn:active:not(:disabled) {
    transform: scale(0.97);
  }
  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .btn-primary {
    background: #2d7cf1;
    border-color: #2d7cf1;
    color: white;
  }
  .btn-primary:hover:not(:disabled) {
    background: #1f6ae0;
    border-color: #1f6ae0;
  }

  .name-field {
    display: flex;
    align-items: center;
    gap: 6px;
    background: white;
    border: 1px solid #d4d4d8;
    border-radius: 5px;
    padding: 0 8px;
    height: 30px;
  }
  .name-field .muted {
    font-size: 11px;
    color: #71717a;
    text-transform: uppercase;
    letter-spacing: 0.08em;
  }
  .name-input {
    border: none;
    outline: none;
    background: transparent;
    font-size: 13px;
    color: #18181b;
    font-family: inherit;
    width: 220px;
  }

  .spacer {
    flex: 1;
  }

  .hint {
    font-size: 11px;
    color: #71717a;
    font-style: italic;
  }
</style>
