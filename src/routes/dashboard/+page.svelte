<!--
  Blueprint Dashboard — `/dashboard`.

  Lists every blueprint tracked in the inventory (filtered: missing
  files are dropped at read-time so the user only sees what's currently
  runnable). Each card shows the cached `name`, last-seen timestamp,
  and the absolute path (truncated).

  Actions per card:
    * Run     — Loads the blueprint JSON, asks for any declared inputs
                in an inline modal (reusing `InputsDialog`), then invokes
                `run_blueprint` and renders the result inline.
    * Open    — Deep-links into the editor at `/?path=<absolute>` so the
                editor picks it up via its `?path=` query param.
    * Remove  — Drops the entry from the inventory without deleting the
                file from disk.

  Top-bar:
    * Add Blueprint — opens a file picker; for the selected blueprint
                     file, reads its `name` and upserts into the
                     inventory.

  Missing-file behaviour:
    `inventory_list` filters the missing-on-disk ones OUT of the
    visible list but keeps them in the underlying file. If the user
    restores the file, it reappears on the next refresh — no manual
    re-add needed.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { invoke } from '@tauri-apps/api/core';
  import { open as openDialog } from '@tauri-apps/plugin-dialog';
  import InputsDialog, { type InputRequest } from '$lib/components/InputsDialog.svelte';
  import type {
    BlueprintInput,
    Blueprint,
    BlueprintMeta,
  } from '$lib/types/blueprint';
  import { migrateBlueprintInput } from '$lib/types/blueprint';
  import type { RunResult } from '$lib/utils/runner-io';
  import { runBlueprint, renderValue } from '$lib/utils/runner-io';
  import { blueprintToFlow } from '$lib/utils/blueprint-io';

  /** Shape returned by the Rust `inventory_list` command. */
  interface InventoryEntry {
    path: string;
    name: string;
    added_at: number;
    last_seen_at: number;
  }

  let entries = $state<InventoryEntry[]>([]);
  let loading = $state(false);
  // Re-entrancy guard for the bulk-add flow. Set true at the start of
  // `handleAdd`, reset in a `finally` so a thrown error still clears
  // the gate. Buttons flip on the same flag so a rapid double-click
  // can't fire two `openDialog` calls in flight.
  let addRunning = $state(false);
  let status = $state<{ message: string; kind: 'info' | 'error' }>({
    message: 'Loading…',
    kind: 'info',
  });
  let inventoryDir = $state<string | null>(null);

  // Run state — one inline modal at a time.
  let runModal = $state<{
    opened: boolean;
    path: string | null;
    name: string;
    inputs: InputRequest[];
    running: boolean;
    result: RunResult | null;
    error: string | null;
  }>({
    opened: false,
    path: null,
    name: '',
    inputs: [],
    running: false,
    result: null,
    error: null,
  });

  onMount(async () => {
    await refresh();
    try {
      inventoryDir = await invoke<string>('inventory_path');
    } catch {
      inventoryDir = null;
    }
  });

  async function refresh() {
    loading = true;
    try {
      entries = await invoke<InventoryEntry[]>('inventory_list');
      status = {
        message:
          entries.length === 0
            ? 'No saved blueprints yet. Click "Add Blueprint" or save one from the Editor.'
            : `${entries.length} blueprint${entries.length === 1 ? '' : 's'} tracked`,
        kind: 'info',
      };
    } catch (err) {
      entries = [];
      status = { message: `Inventory read failed: ${err}`, kind: 'error' };
    } finally {
      loading = false;
    }
  }

  function fmtTimestamp(unixSeconds: number): string {
    if (!unixSeconds) return '—';
    try {
      const d = new Date(unixSeconds * 1000);
      return d.toLocaleString(undefined, {
        year: 'numeric',
        month: '2-digit',
        day: '2-digit',
        hour: '2-digit',
        minute: '2-digit',
      });
    } catch {
      return '—';
    }
  }

  function shortPath(p: string): string {
    if (p.length <= 60) return p;
    return p.slice(0, 24) + '…' + p.slice(-32);
  }

  async function handleAdd() {
    // Re-entrancy guard: `addRunning` is gated at the top AND wired to
    // the button's `disabled` so a rapid double-click can't fire two
    // `openDialog` calls in flight whose results would interleave.
    if (addRunning) return;
    addRunning = true;
    try {
      // `multiple: true` lets the user pick a whole folder's worth of
      // blueprint JSONs in one go. The native dialog returns `string[]`
      // for multi-select; we accept a `string` defensively in case the
      // plugin shape ever drops back to single (graceful).
      const picked = await openDialog({
        title: 'Add saved blueprints to library',
        multiple: true,
        filters: [
          { name: 'Blueprint JSON', extensions: ['blueprint.json', 'json'] },
        ],
      });
      if (!picked) return;
      const paths = Array.isArray(picked) ? picked : [picked];
      if (paths.length === 0) return;

      // Parallelise per-path: the Rust `inventory_add` is internally
      // serialised by the inventory-mod Mutex so concurrent invokes
      // can't corrupt the JSON file. `Promise.allSettled` keeps the
      // per-path error shape identical to the sequential fallback —
      // each path is reported individually rather than short-circuiting
      // on the first rejection.
      const results = await Promise.allSettled(
        paths.map(async (path) => {
          const json = await invoke<string>('load_blueprint', { path });
          const parsed = JSON.parse(json) as { name?: string };
          await invoke('inventory_add', {
            path,
            name: parsed.name ?? path.split(/[\\/]/).pop() ?? 'Untitled',
          });
        }),
      );

      let successCount = 0;
      const errors: Array<{ path: string; message: string }> = [];
      results.forEach((r, i) => {
        if (r.status === 'fulfilled') {
          successCount += 1;
        } else {
          errors.push({ path: paths[i], message: String(r.reason) });
        }
      });

      await refresh();

      // Three-band status reporting: all-ok / mixed / all-fail.
      if (errors.length === 0) {
        status = {
          message:
            successCount === 1
              ? `Added → ${paths[0]}`
              : `Added ${successCount} blueprints`,
          kind: 'info',
        };
      } else if (successCount === 0) {
        status = {
          message: `Add failed for ${paths.length} file${paths.length === 1 ? '' : 's'}: ${errors[0]?.message ?? 'unknown error'}`,
          kind: 'error',
        };
      } else {
        status = {
          message: `Added ${successCount}/${paths.length}; ${errors.length} failed: ${errors[0]?.message ?? 'unknown error'}`,
          kind: 'error',
        };
      }
    } finally {
      addRunning = false;
    }
  }

  function handleOpen(entry: InventoryEntry) {
    void goto(`/?path=${encodeURIComponent(entry.path)}`);
  }

  async function handleRun(entry: InventoryEntry) {
    runModal = {
      opened: true,
      path: entry.path,
      name: entry.name,
      inputs: [],
      running: false,
      result: null,
      error: null,
    };
    try {
      const json = await invoke<string>('load_blueprint', { path: entry.path });
      const parsed = JSON.parse(json) as {
        metadata?: { inputs?: BlueprintInput[] };
      };
      const inputs = (parsed.metadata?.inputs ?? []).map(
        (inp, i): InputRequest => {
          // Run the legacy `'path' → 'file'` migration so an older
          // blueprint (pre-picker-split) opens a file picker instead
          // of falling through to InputsDialog's JSON textarea
          // branch with a `<select>` mismatch.
          const migrated = migrateBlueprintInput(inp);
          return {
            name: migrated.id,
            label: migrated.name || migrated.id,
            // `InputRequest.type` is now `BlueprintInput['type']`
            // directly, so we pass `migrated.type` as-is — the
            // dialog has explicit branches for `file`, `folder`,
            // `multi-file` (native pickers + chip list) and falls
            // back to a JSON textarea for any unknown value.
            type: migrated.type,
            nodeId: `dash:${i}:${migrated.id}`,
          };
        },
      );
      runModal = { ...runModal, inputs };
      if (inputs.length === 0) {
        // No inputs → kick off immediately, don't show the modal.
        await executeRun({});
      }
    } catch (err) {
      runModal = {
        ...runModal,
        error: String(err),
      };
    }
  }

  async function executeRun(values: Record<string, unknown>) {
    if (!runModal.path) return;
    runModal = { ...runModal, running: true, error: null, result: null };
    try {
      const json = await invoke<string>('load_blueprint', { path: runModal.path });
      // Parse once — the JSON drives both the runtime inputs list
      // AND the run helper. Round-trip through `blueprintToFlow` for
      // typed nodes/edges so `runBlueprint` sees the same FlowNode /
      // FlowEdge shape the editor would.
      const parsed = JSON.parse(json) as Blueprint;
      const flow = blueprintToFlow(parsed);
      const meta: BlueprintMeta = {
        id: parsed.id,
        name: parsed.name,
        description: parsed.description,
        schema_version: 1 as const,
        metadata: parsed.metadata,
      };
      const result = await runBlueprint(
        meta,
        flow.nodes,
        flow.edges,
        values,
      );
      runModal = { ...runModal, running: false, result };
    } catch (err) {
      runModal = { ...runModal, running: false, error: String(err) };
    }
  }

  function handleRunCancel() {
    runModal = {
      opened: false,
      path: null,
      name: '',
      inputs: [],
      running: false,
      result: null,
      error: null,
    };
  }

  async function handleRemove(entry: InventoryEntry) {
    try {
      await invoke('inventory_remove', { path: entry.path });
      await refresh();
      status = {
        message: `Removed from library: ${entry.name}`,
        kind: 'info',
      };
    } catch (err) {
      status = { message: `Remove failed: ${err}`, kind: 'error' };
    }
  }
</script>

<div class="dashboard">
  <header class="dash-header">
    <div class="title-block">
      <h1>Blueprint Library</h1>
      <p>{status.message}</p>
    </div>
    <div class="actions">
      <button class="btn" type="button" onclick={refresh} disabled={loading}>
        {loading ? 'Refreshing…' : 'Refresh'}
      </button>
      <button
        class="btn btn-primary"
        type="button"
        onclick={handleAdd}
        disabled={addRunning}
      >
        {addRunning ? 'Adding…' : '+ Add Blueprint'}
      </button>
    </div>
  </header>

  {#if inventoryDir}
    <div class="path-line">
      Library: <code>{inventoryDir}</code>
    </div>
  {/if}

  <main class="grid">
    {#if entries.length === 0}
      <div class="empty">
        {#if loading}
          <p>Loading…</p>
        {:else}
          <h2>Your library is empty</h2>
          <p>
            Save a blueprint from the Editor (its path is added
            automatically) or click
            <strong>Add Blueprint</strong> to track an existing file.
          </p>
        {/if}
      </div>
    {:else}
      {#each entries as entry (entry.path)}
        <article class="card">
          <header class="card-head">
            <h3 title={entry.name}>{entry.name}</h3>
            <span class="path" title={entry.path}>{shortPath(entry.path)}</span>
          </header>
          <dl class="meta">
            <div>
              <dt>Last seen</dt>
              <dd>{fmtTimestamp(entry.last_seen_at)}</dd>
            </div>
            <div>
              <dt>Added</dt>
              <dd>{fmtTimestamp(entry.added_at)}</dd>
            </div>
          </dl>
          <div class="card-actions">
            <button class="btn run" type="button" onclick={() => handleRun(entry)}>
              ▶ Run
            </button>
            <button class="btn open" type="button" onclick={() => handleOpen(entry)}>
              Open
            </button>
            <button
              class="btn remove"
              type="button"
              title="Remove from library (file on disk is untouched)"
              onclick={() => handleRemove(entry)}
            >
              Remove
            </button>
          </div>
        </article>
      {/each}
    {/if}
  </main>
</div>

<!-- Inline Run modal. Renders InputsDialog when meta.inputs > 0;
     otherwise result panel renders immediately. Closes on cancel OR
     on a final success/failure result. -->
{#if runModal.opened}
  {#if runModal.inputs.length > 0 && !runModal.running && !runModal.result && !runModal.error}
    <InputsDialog
      inputs={runModal.inputs}
      storageKey={runModal.path}
      onSubmit={(values) => void executeRun(values)}
      onCancel={handleRunCancel}
    />
  {/if}
  {#if runModal.running || runModal.result || runModal.error}
    <div class="run-panel-backdrop" onclick={handleRunCancel} role="presentation">
      <div
        class="run-panel"
        role="dialog"
        aria-modal="true"
        onclick={(e) => e.stopPropagation()}
      >
        <header>
          <h3>Run · {runModal.name}</h3>
          <button class="close" type="button" onclick={handleRunCancel}>×</button>
        </header>
        <div class="run-body">
          {#if runModal.running}
            <p class="muted">Running…</p>
          {:else if runModal.error}
            <h4>Error</h4>
            <pre class="error">{runModal.error}</pre>
          {:else if runModal.result}
            {@const r = runModal.result}
            <h4>Output</h4>
            <dl class="kv">
              {#each Object.entries(r.outputs) as [k, v] (k)}
                <div>
                  <dt>{k}</dt>
                  <dd><code>{renderValue(v)}</code></dd>
                </div>
              {:else}
                <span class="muted">(no outputs)</span>
              {/each}
            </dl>
            <h4>Logs</h4>
            {#if r.logs.length === 0}
              <span class="muted">(no log entries)</span>
            {:else}
              <ul class="logs">
                {#each r.logs as log, i (i)}
                  <li><span class="log-label">{log.label}</span>: <code>{renderValue(log.value)}</code></li>
                {/each}
              </ul>
            {/if}
            <p class="stats muted">
              {r.stats.nodes_executed} nodes executed in {r.stats.duration_ms} ms
            </p>
          {/if}
        </div>
      </div>
    </div>
  {/if}
{/if}

<style>
  .dashboard {
    height: 100vh;
    display: flex;
    flex-direction: column;
    padding: 56px 32px 24px;
    background: #f4f4f5;
    font-family: 'Inter', system-ui, sans-serif;
    overflow: hidden;
  }

  .dash-header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 16px;
    margin-bottom: 8px;
  }
  .dash-header h1 {
    margin: 0;
    font-size: 22px;
    font-weight: 700;
    color: #18181b;
    letter-spacing: 0.01em;
  }
  .dash-header p {
    margin: 4px 0 0;
    font-size: 12.5px;
    color: #71717a;
  }
  .actions {
    display: flex;
    gap: 8px;
  }
  .btn {
    height: 32px;
    padding: 0 14px;
    background: white;
    border: 1px solid #d4d4d8;
    border-radius: 5px;
    color: #27272a;
    font-size: 12.5px;
    font-weight: 500;
    cursor: pointer;
    font-family: inherit;
    transition:
      background 0.1s ease,
      border-color 0.1s ease,
      color 0.1s ease,
      transform 0.05s ease;
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

  .path-line {
    font-family: 'JetBrains Mono', monospace;
    font-size: 11px;
    color: #71717a;
    margin-bottom: 12px;
  }
  .path-line code {
    background: white;
    border: 1px solid #e4e4e7;
    border-radius: 3px;
    padding: 1px 6px;
    color: #3f3f46;
  }

  .grid {
    flex: 1;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
    gap: 14px;
    overflow-y: auto;
    padding: 4px 0 24px;
    align-content: start;
  }

  .empty {
    grid-column: 1 / -1;
    background: white;
    border: 1px dashed #d4d4d8;
    border-radius: 8px;
    padding: 40px 24px;
    text-align: center;
    color: #71717a;
  }
  .empty h2 {
    margin: 0 0 6px;
    font-size: 16px;
    color: #3f3f46;
    font-weight: 600;
  }
  .empty p {
    margin: 0;
    font-size: 12.5px;
  }

  .card {
    background: white;
    border: 1px solid #e4e4e7;
    border-radius: 7px;
    box-shadow: 0 1px 2px rgba(0, 0, 0, 0.04);
    padding: 14px 14px 10px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    transition:
      border-color 0.12s ease,
      box-shadow 0.12s ease,
      transform 0.06s ease;
  }
  .card:hover {
    border-color: #2d7cf1;
    box-shadow: 0 2px 6px rgba(45, 124, 241, 0.12);
  }
  .card-head h3 {
    margin: 0;
    font-size: 13.5px;
    font-weight: 600;
    color: #18181b;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .card-head .path {
    display: block;
    font-family: 'JetBrains Mono', monospace;
    font-size: 10.5px;
    color: #71717a;
    background: #f4f4f5;
    border: 1px solid #e4e4e7;
    border-radius: 3px;
    padding: 1px 5px;
    margin-top: 6px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 100%;
    display: block;
  }
  .meta {
    margin: 0;
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 6px 12px;
    font-family: 'JetBrains Mono', monospace;
    font-size: 11px;
  }
  .meta div {
    margin: 0;
  }
  .meta dt {
    color: #71717a;
    font-weight: 500;
    margin: 0;
  }
  .meta dd {
    color: #3f3f46;
    margin: 0;
  }

  .card-actions {
    display: flex;
    gap: 6px;
    align-items: center;
    margin-top: auto;
  }
  .card-actions .run {
    flex: 1;
    background: #16a34a;
    border-color: #16a34a;
    color: white;
    font-weight: 600;
  }
  .card-actions .run:hover {
    background: #15803d;
    border-color: #15803d;
  }
  .card-actions .open {
    flex: 0 0 auto;
  }
  .card-actions .remove {
    flex: 0 0 auto;
    color: #71717a;
  }
  .card-actions .remove:hover {
    background: #fee2e2;
    border-color: #ef4444;
    color: #ef4444;
  }

  .run-panel-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(15, 23, 42, 0.42);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 50;
    backdrop-filter: blur(1px);
  }
  .run-panel {
    background: white;
    border-radius: 9px;
    box-shadow: 0 16px 40px rgba(0, 0, 0, 0.28);
    width: min(560px, 92vw);
    max-height: 80vh;
    display: flex;
    flex-direction: column;
    border: 1px solid #d4d4d8;
    overflow: hidden;
    font-family: 'Inter', system-ui, sans-serif;
  }
  .run-panel header {
    padding: 14px 18px 10px;
    border-bottom: 1px solid #e4e4e7;
    background: #fafafa;
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .run-panel h3 {
    margin: 0;
    font-size: 14px;
    font-weight: 700;
    color: #18181b;
  }
  .run-panel .close {
    background: transparent;
    border: none;
    font-size: 20px;
    line-height: 1;
    color: #71717a;
    cursor: pointer;
    padding: 0 6px;
    border-radius: 4px;
  }
  .run-panel .close:hover {
    background: #f4f4f5;
    color: #18181b;
  }
  .run-body {
    padding: 14px 18px;
    overflow-y: auto;
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .run-body h4 {
    margin: 8px 0 4px;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: #52525b;
  }
  .run-body .kv {
    margin: 0;
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 4px 12px;
    font-family: 'JetBrains Mono', monospace;
    font-size: 11.5px;
  }
  .run-body .kv dt {
    color: #52525b;
    font-weight: 600;
  }
  .run-body .kv dd {
    margin: 0;
    color: #18181b;
  }
  .run-body .kv code {
    font-family: inherit;
    background: #f4f4f5;
    padding: 0 4px;
    border-radius: 2px;
    border: 1px solid #e4e4e7;
  }
  .run-body .error {
    background: #fef2f2;
    border: 1px solid #fecaca;
    color: #b91c1c;
    padding: 10px;
    border-radius: 4px;
    font-family: 'JetBrains Mono', monospace;
    font-size: 11.5px;
    margin: 0;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .run-body .logs {
    list-style: none;
    padding: 0;
    margin: 0;
    font-family: 'JetBrains Mono', monospace;
    font-size: 11px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .run-body .logs li {
    background: #f4f4f5;
    border: 1px solid #e4e4e7;
    border-radius: 3px;
    padding: 3px 8px;
    color: #3f3f46;
  }
  .run-body .stats {
    margin: 8px 0 0;
    font-family: 'JetBrains Mono', monospace;
    font-size: 10.5px;
    color: #71717a;
  }
  .muted {
    color: #71717a;
    font-style: italic;
    font-size: 11.5px;
    margin: 4px 0;
  }
</style>
