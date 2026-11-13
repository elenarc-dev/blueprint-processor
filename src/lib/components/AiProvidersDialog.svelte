<!--
  Global AI providers: endpoints, model lists and API keys.

  Providers are configured ONCE here, not per node — anything in a node's
  `params` is written into the `.blueprint.json`, which users save, diff
  and share. The Rust side stores everything in
  `<app_data_dir>/secrets.json` with owner-only permissions (see
  src-tauri/src/ai/secrets.rs). This dialog is the only place a key ever
  enters the app.

  The backend never returns a key — `ai_providers` reports `configured`
  plus a last-4 `hint`, so the full secret never comes back over the IPC
  bridge (AI-SEC-002). Saving with an empty key field keeps the stored
  key; use "Clear key" to actually drop it.

  A provider's `id` is the stable handle nodes use in the Model dropdown
  (`ollama::llama3.1:70b`); renaming only changes the label.
-->
<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import {
    KIND_LABELS,
    PROVIDER_KINDS,
    providerName,
    refreshAiProviders,
    setAiProviders,
    type ProviderDraft,
    type ProviderStatus,
  } from '$lib/utils/ai-config.svelte';

  interface Props {
    open: boolean;
  }
  let { open = $bindable(false) }: Props = $props();

  /**
   * One editable row: the draft shape (`id`…`apiKey`, `models` flattened to
   * a textarea) plus the read-only `configured`/`hint` the backend reported,
   * which drive the key pill and the key-field placeholder.
   */
  interface Row {
    id: string;
    kind: string;
    label: string;
    baseUrl: string;
    modelsText: string;
    /**
     * Always starts empty: the backend never sends a key back, and an empty
     * field means "keep what is stored".
     */
    apiKey: string;
    configured: boolean;
    hint: string | null;
  }

  const NEW_ID = '__new__';

  function emptyRow(): Row {
    return {
      id: '',
      kind: PROVIDER_KINDS[0],
      label: '',
      baseUrl: '',
      modelsText: '',
      apiKey: '',
      configured: false,
      hint: null,
    };
  }

  let rows = $state<Row[]>([]);
  // One stable object for the "new provider" form: recreating it per render
  // would throw away whatever the user typed.
  let draft = $state<Row>(emptyRow());
  let adding = $state(false);
  let busy = $state<string | null>(null);
  let confirmRemove = $state<string | null>(null);
  let message = $state<{ text: string; kind: 'info' | 'error' } | null>(null);

  // Re-read every time the dialog opens so providers added on another
  // window (or edited by hand in secrets.json) show the real state.
  $effect(() => {
    if (open) void openDialog();
  });

  async function openDialog() {
    const statuses = await refreshAiProviders();
    rows = statuses.map(rowOf);
    draft = emptyRow();
    adding = false;
    confirmRemove = null;
    message = null;
  }

  function rowOf(s: ProviderStatus): Row {
    return {
      id: s.id,
      kind: s.kind,
      label: s.label,
      baseUrl: s.baseUrl,
      modelsText: s.models.join('\n'),
      apiKey: '',
      configured: s.configured,
      hint: s.hint,
    };
  }

  function draftOf(row: Row): ProviderDraft {
    return {
      id: row.id,
      kind: row.kind,
      label: row.label,
      baseUrl: row.baseUrl,
      models: row.modelsText.split('\n'),
      apiKey: row.apiKey,
    };
  }

  /** Replace the row list with the statuses the backend just returned. */
  async function apply(statuses: ProviderStatus[], text: string) {
    setAiProviders(statuses);
    rows = statuses.map(rowOf);
    message = { text, kind: 'info' };
  }

  async function save(row: Row, isNew: boolean) {
    const target = isNew ? NEW_ID : row.id;
    busy = target;
    try {
      const saved = await invoke<ProviderStatus[]>('ai_save_provider', {
        draft: draftOf(row),
      });
      if (isNew) adding = false;
      await apply(saved, isNew ? 'Provider added.' : `Saved ${row.id || 'provider'}.`);
    } catch (err) {
      message = { text: String(err), kind: 'error' };
    } finally {
      busy = null;
    }
  }

  async function clearKey(row: Row) {
    busy = row.id;
    try {
      const saved = await invoke<ProviderStatus[]>('ai_clear_provider_key', { id: row.id });
      await apply(saved, `Key removed from ${row.id}.`);
    } catch (err) {
      message = { text: String(err), kind: 'error' };
    } finally {
      busy = null;
    }
  }

  async function remove(row: Row) {
    // Two-click confirm: this dialog has no modal layer of its own.
    if (confirmRemove !== row.id) {
      confirmRemove = row.id;
      return;
    }
    busy = row.id;
    try {
      const saved = await invoke<ProviderStatus[]>('ai_remove_provider', { id: row.id });
      confirmRemove = null;
      await apply(saved, `Removed ${row.id}.`);
    } catch (err) {
      message = { text: String(err), kind: 'error' };
    } finally {
      busy = null;
    }
  }

  function addProvider() {
    draft = emptyRow();
    adding = true;
    confirmRemove = null;
    message = null;
  }

  function cancelAdd() {
    adding = false;
    draft = emptyRow();
  }

  function close() {
    open = false;
    message = null;
  }
</script>

{#if open}
  <div class="backdrop"></div>
  <div class="panel">
    <h3>AI providers</h3>
    <p class="muted">
      Providers and their keys are configured once, on this machine only,
      in the app data directory with owner-only permissions. Nodes only
      pick a model from this list — nothing here is ever written into a
      blueprint, so graphs that use AI nodes stay safe to share.
    </p>

    {#each rows as row (row.id)}
      <div class="provider">
        <div class="provider-head">
          <code class="pid">{row.id}</code>
          <span class="muted kind">{KIND_LABELS[row.kind] ?? row.kind}</span>
          {#if row.configured}
            <span class="pill ok">key set {row.hint ?? ''}</span>
          {:else if row.baseUrl}
            <span class="pill">local endpoint, no key</span>
          {:else}
            <span class="pill">no key — add one, or a Base URL</span>
          {/if}
        </div>

        <div class="grid">
          <label>
            <span>Name</span>
            <input type="text" placeholder={row.id} bind:value={row.label} />
          </label>
          <label>
            <span>Kind</span>
            <select bind:value={row.kind}>
              {#each PROVIDER_KINDS as kind (kind)}
                <option value={kind}>{KIND_LABELS[kind] ?? kind}</option>
              {/each}
            </select>
          </label>
          <label class="wide">
            <span>Base URL (empty = provider default endpoint; set it for Ollama, LM Studio, Groq…)</span>
            <input
              type="text"
              placeholder="http://127.0.0.1:11434/v1"
              spellcheck="false"
              bind:value={row.baseUrl}
            />
          </label>
          <label class="wide">
            <span>Models (one per line — offered in each node's Model dropdown)</span>
            <textarea
              rows={3}
              placeholder={'gpt-4o-mini\ngpt-4o'}
              spellcheck="false"
              bind:value={row.modelsText}
            ></textarea>
          </label>
          <label class="wide">
            <span>API key</span>
            <input
              type="password"
              placeholder={row.configured
                ? `stored ${row.hint ?? ''} — type to replace`
                : 'Paste API key…'}
              autocomplete="off"
              spellcheck="false"
              bind:value={row.apiKey}
            />
          </label>
        </div>

        <div class="provider-row">
          <button class="btn primary" onclick={() => save(row, false)} disabled={busy === row.id}>
            {busy === row.id ? 'Saving…' : 'Save'}
          </button>
          <button
            class="btn"
            onclick={() => clearKey(row)}
            disabled={!row.configured || busy === row.id}
          >
            Clear key
          </button>
          <button class="btn danger" onclick={() => remove(row)} disabled={busy === row.id}>
            {confirmRemove === row.id ? 'Really remove?' : 'Remove'}
          </button>
        </div>
      </div>
    {/each}

    {#if adding}
      <div class="provider new">
        <div class="provider-head">
          <strong>New provider</strong>
          <span class="muted kind">
            its id is generated from the Kind on save (e.g.
            <code>openai</code>, <code>openai-2</code>) and never changes
          </span>
        </div>
        <div class="grid">
          <label>
            <span>Name</span>
            <input type="text" placeholder="Local Ollama" bind:value={draft.label} />
          </label>
          <label>
            <span>Kind</span>
            <select bind:value={draft.kind}>
              {#each PROVIDER_KINDS as kind (kind)}
                <option value={kind}>{KIND_LABELS[kind] ?? kind}</option>
              {/each}
            </select>
          </label>
          <label class="wide">
            <span>Base URL (empty = provider default endpoint; set it for Ollama, LM Studio, Groq…)</span>
            <input
              type="text"
              placeholder="http://127.0.0.1:11434/v1"
              spellcheck="false"
              bind:value={draft.baseUrl}
            />
          </label>
          <label class="wide">
            <span>Models (one per line — offered in each node's Model dropdown)</span>
            <textarea
              rows={3}
              placeholder={'llama3.1:70b\nqwen2.5-coder:32b'}
              spellcheck="false"
              bind:value={draft.modelsText}
            ></textarea>
          </label>
          <label class="wide">
            <span>API key (leave empty for a local server that needs none)</span>
            <input
              type="password"
              placeholder="Paste API key…"
              autocomplete="off"
              spellcheck="false"
              bind:value={draft.apiKey}
            />
          </label>
        </div>
        <div class="provider-row">
          <button class="btn primary" onclick={() => save(draft, true)} disabled={busy === NEW_ID}>
            {busy === NEW_ID ? 'Saving…' : 'Add provider'}
          </button>
          <button class="btn" onclick={cancelAdd}>Cancel</button>
        </div>
      </div>
    {:else}
      <div class="provider-row add">
        <button class="btn" onclick={addProvider}>Add provider</button>
      </div>
    {/if}

    {#if message}
      <p class:error={message.kind === 'error'} class="status">{message.text}</p>
    {/if}

    <div class="actions">
      <button class="btn" onclick={close}>Close</button>
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(24, 24, 27, 0.35);
    z-index: 40;
  }
  .panel {
    position: fixed;
    top: 72px;
    left: 50%;
    transform: translateX(-50%);
    width: min(560px, calc(100vw - 32px));
    max-height: calc(100vh - 120px);
    overflow-y: auto;
    background: white;
    border: 1px solid #d4d4d8;
    border-radius: 8px;
    box-shadow: 0 18px 40px rgba(0, 0, 0, 0.18);
    padding: 16px 18px;
    z-index: 41;
    font-family: 'Inter', system-ui, sans-serif;
    color: #18181b;
  }
  h3 {
    margin: 0 0 6px;
    font-size: 14px;
  }
  .muted {
    margin: 0 0 14px;
    font-size: 11.5px;
    line-height: 1.45;
    color: #52525b;
  }
  .muted.kind {
    margin: 0;
  }
  .provider {
    border-top: 1px solid #f0f0f3;
    padding: 12px 0;
  }
  .provider.new {
    background: #fbfbfd;
  }
  .provider-head {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 8px;
    font-size: 12.5px;
  }
  .pid {
    font-size: 12px;
    padding: 1px 6px;
    border-radius: 4px;
    background: #f4f4f5;
    border: 1px solid #e4e4e7;
  }
  .grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 8px;
  }
  .grid label {
    display: flex;
    flex-direction: column;
    gap: 3px;
    font-size: 11px;
    color: #52525b;
  }
  .grid .wide {
    grid-column: 1 / -1;
  }
  .grid input,
  .grid select,
  .grid textarea {
    min-height: 28px;
    border: 1px solid #d4d4d8;
    border-radius: 4px;
    padding: 4px 8px;
    font-size: 12px;
    font-family: inherit;
    background: white;
    width: 100%;
    box-sizing: border-box;
  }
  .grid textarea {
    resize: vertical;
  }
  .grid input:focus,
  .grid select:focus,
  .grid textarea:focus {
    outline: none;
    border-color: #2d7cf1;
    box-shadow: 0 0 0 2px rgba(45, 124, 241, 0.2);
  }
  .provider-row {
    display: flex;
    justify-content: flex-end;
    gap: 6px;
    margin-top: 8px;
  }
  .provider-row.add {
    justify-content: flex-start;
    border-top: 1px solid #f0f0f3;
    padding-top: 12px;
  }
  .pill {
    font-size: 10.5px;
    padding: 1px 6px;
    border-radius: 999px;
    background: #f4f4f5;
    color: #52525b;
    border: 1px solid #e4e4e7;
    margin-left: auto;
  }
  .pill.ok {
    background: #ecfdf5;
    color: #047857;
    border-color: #a7f3d0;
  }
  .btn {
    height: 28px;
    padding: 0 10px;
    background: white;
    border: 1px solid #d4d4d8;
    border-radius: 4px;
    font-size: 12px;
    font-family: inherit;
    cursor: pointer;
  }
  .btn:hover:not(:disabled) {
    background: #f4f4f5;
  }
  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .btn.primary {
    border-color: #2d7cf1;
    color: #1d4ed8;
  }
  .btn.primary:hover:not(:disabled) {
    background: #eff6ff;
  }
  .btn.danger:hover:not(:disabled) {
    background: #fef2f2;
    border-color: #fca5a5;
    color: #b91c1c;
  }
  .status {
    margin: 8px 0 0;
    font-size: 11.5px;
    color: #047857;
  }
  .status.error {
    color: #b91c1c;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    margin-top: 14px;
  }
</style>
