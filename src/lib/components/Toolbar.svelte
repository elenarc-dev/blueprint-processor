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
  import type { Blueprint } from '$lib/types/blueprint';
  import { makeMeta, type BlueprintMeta } from '$lib/types/blueprint';
  import AiKeysDialog from './AiKeysDialog.svelte';
  import {
    blueprintToFlow,
    composeBlueprint,
    type FlowNode,
    type FlowEdge,
  } from '$lib/utils/blueprint-io';

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
  }
  let {
    meta,
    nodes,
    edges,
    currentFilePath,
    onMetaChange,
    onPathChange,
    onAfterIo,
  }: Props = $props();

  let saving = $state(false);
  let loading = $state(false);
  /** AI provider keys live here, not in a node — see AiKeysDialog. */
  let keysOpen = $state(false);

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
      onAfterIo(`Saved ${result.bytes} B → ${result.path}`, 'info');
    } catch (err) {
      onAfterIo(`Save As failed: ${err}`, 'error');
    } finally {
      saving = false;
    }
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

  <div class="separator"></div>

  <button
    class="btn"
    onclick={() => (keysOpen = true)}
    title="Configure AI provider API keys"
  >
    AI Keys
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

<AiKeysDialog bind:open={keysOpen} />

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
