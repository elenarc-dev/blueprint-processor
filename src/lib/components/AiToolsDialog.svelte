<!--
  App-level AI tools: named HTTP endpoints an `ai.completion` node can
  offer to the model.

  Like AI providers, tools are global machine config — never blueprint
  content — so a saved graph carries only the ids it selected. The backend
  stores them in `<app_data_dir>/tools.json` with owner-only permissions
  (headers may hold a token); see src-tauri/src/app_tools.rs.

  A tool call POSTs the model's arguments as JSON to `URL` and returns
  `{ status, body }`. `Parameters` is the JSON Schema the model sees.
-->
<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import {
    refreshAiTools,
    setAiTools,
    type AppTool,
  } from '$lib/utils/ai-tools.svelte';

  interface Props {
    open: boolean;
  }
  let { open = $bindable(false) }: Props = $props();

  const METHODS = ['POST', 'GET', 'PUT', 'PATCH', 'DELETE'];

  interface Row {
    id: string;
    name: string;
    description: string;
    parametersText: string;
    method: string;
    url: string;
    headers: string;
  }

  function emptyRow(): Row {
    return {
      id: '',
      name: '',
      description: '',
      parametersText: '{"type":"object","properties":{}}',
      method: 'POST',
      url: '',
      headers: '',
    };
  }

  function rowOf(tool: AppTool): Row {
    return {
      id: tool.id,
      name: tool.name,
      description: tool.description,
      parametersText: JSON.stringify(tool.parameters ?? { type: 'object', properties: {} }, null, 2),
      method: tool.method || 'POST',
      url: tool.url,
      headers: tool.headers,
    };
  }

  let rows = $state<Row[]>([]);
  let draft = $state<Row>(emptyRow());
  let adding = $state(false);
  let busy = $state<string | null>(null);
  let confirmRemove = $state<string | null>(null);
  let message = $state<{ text: string; kind: 'info' | 'error' } | null>(null);

  $effect(() => {
    if (open) void openDialog();
  });

  async function openDialog() {
    rows = (await refreshAiTools()).map(rowOf);
    draft = emptyRow();
    adding = false;
    confirmRemove = null;
    message = null;
  }

  function apply(next: AppTool[], text: string) {
    setAiTools(next);
    rows = next.map(rowOf);
    message = { text, kind: 'info' };
  }

  async function save(row: Row, isNew: boolean) {
    let parameters: unknown;
    try {
      parameters = JSON.parse(row.parametersText || '{}');
    } catch (err) {
      message = { text: `Parameters must be valid JSON: ${err}`, kind: 'error' };
      return;
    }
    const target = isNew ? '__new__' : row.id;
    busy = target;
    try {
      const saved = await invoke<AppTool[]>('tools_save', {
        tool: {
          id: row.id,
          name: row.name,
          description: row.description,
          parameters,
          method: row.method,
          url: row.url,
          headers: row.headers,
        },
      });
      if (isNew) adding = false;
      apply(saved, isNew ? 'Tool added.' : `Saved ${row.name || row.id}.`);
    } catch (err) {
      message = { text: String(err), kind: 'error' };
    } finally {
      busy = null;
    }
  }

  async function remove(row: Row) {
    if (confirmRemove !== row.id) {
      confirmRemove = row.id;
      return;
    }
    busy = row.id;
    try {
      const saved = await invoke<AppTool[]>('tools_remove', { id: row.id });
      confirmRemove = null;
      apply(saved, `Removed ${row.name || row.id}.`);
    } catch (err) {
      message = { text: String(err), kind: 'error' };
    } finally {
      busy = null;
    }
  }

  function startAdd() {
    draft = emptyRow();
    adding = true;
    confirmRemove = null;
    message = null;
  }

  function close() {
    open = false;
    message = null;
  }
</script>

{#if open}
  <div class="backdrop" role="presentation" onclick={close}></div>
  <div class="panel">
    <h3>AI tools</h3>
    <p class="muted">
      Tools are defined once, on this machine, and offered to a node by id
      (Inspector → App tools). The model supplies arguments as JSON; the
      app calls the endpoint and returns <code>{'{ status, body }'}</code>.
      Nothing here is written into a blueprint.
    </p>

    {#if message}
      <p class:error={message.kind === 'error'} class="message">{message.text}</p>
    {/if}

    <div class="rows">
      {#each rows as row (row.id)}
        <div class="row-card">
          <div class="row-grid">
            <label>
              <span>Name</span>
              <input type="text" bind:value={row.name} placeholder="find_customer" />
            </label>
            <label>
              <span>Method</span>
              <select bind:value={row.method}>
                {#each METHODS as m (m)}
                  <option value={m}>{m}</option>
                {/each}
              </select>
            </label>
          </div>
          <label>
            <span>URL</span>
            <input type="text" bind:value={row.url} placeholder="https://api.example.com/customers" />
          </label>
          <label>
            <span>Description (shown to the model)</span>
            <input type="text" bind:value={row.description} placeholder="Look up a customer by email." />
          </label>
          <label>
            <span>Parameters (JSON Schema)</span>
            <textarea rows="3" spellcheck="false" bind:value={row.parametersText}></textarea>
          </label>
          <label>
            <span>Headers (one <code>Name: Value</code> per line)</span>
            <textarea rows="2" spellcheck="false" bind:value={row.headers}></textarea>
          </label>
          <div class="row-actions">
            <span class="id-pill">{row.id}</span>
            <button class="btn" onclick={() => save(row, false)} disabled={busy !== null}>
              {busy === row.id ? 'Saving…' : 'Save'}
            </button>
            <button class="btn danger" onclick={() => remove(row)} disabled={busy !== null}>
              {confirmRemove === row.id ? 'Confirm remove' : 'Remove'}
            </button>
          </div>
        </div>
      {/each}
    </div>

    {#if adding}
      <div class="row-card draft">
        <div class="row-grid">
          <label>
            <span>Name</span>
            <input type="text" bind:value={draft.name} placeholder="find_customer" />
          </label>
          <label>
            <span>Method</span>
            <select bind:value={draft.method}>
              {#each METHODS as m (m)}
                <option value={m}>{m}</option>
              {/each}
            </select>
          </label>
        </div>
        <label>
          <span>URL</span>
          <input type="text" bind:value={draft.url} placeholder="https://api.example.com/customers" />
        </label>
        <label>
          <span>Description (shown to the model)</span>
          <input type="text" bind:value={draft.description} placeholder="Look up a customer by email." />
        </label>
        <label>
          <span>Parameters (JSON Schema)</span>
          <textarea rows="3" spellcheck="false" bind:value={draft.parametersText}></textarea>
        </label>
        <label>
          <span>Headers (one <code>Name: Value</code> per line)</span>
          <textarea rows="2" spellcheck="false" bind:value={draft.headers}></textarea>
        </label>
        <div class="row-actions">
          <button class="btn primary" onclick={() => save(draft, true)} disabled={busy !== null}>
            {busy === '__new__' ? 'Adding…' : 'Add tool'}
          </button>
          <button class="btn" onclick={() => (adding = false)}>Cancel</button>
        </div>
      </div>
    {/if}

    <div class="footer">
      <button class="btn primary" onclick={startAdd} disabled={adding}>+ Add tool</button>
      <button class="btn" onclick={close}>Close</button>
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.35);
    z-index: 50;
  }
  .panel {
    position: fixed;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    width: min(720px, calc(100vw - 32px));
    max-height: calc(100vh - 64px);
    overflow-y: auto;
    background: #fafafa;
    border: 1px solid #d4d4d8;
    border-radius: 10px;
    box-shadow: 0 16px 48px rgba(0, 0, 0, 0.25);
    padding: 16px 18px 18px;
    z-index: 51;
    font-family: 'Inter', system-ui, sans-serif;
    color: #18181b;
  }
  h3 {
    margin: 0 0 6px;
    font-size: 15px;
  }
  .muted {
    margin: 0 0 12px;
    font-size: 12px;
    line-height: 1.5;
    color: #52525b;
  }
  .muted code,
  label code {
    font-family: 'JetBrains Mono', monospace;
    font-size: 10.5px;
    background: #f4f4f5;
    border: 1px solid #e4e4e7;
    border-radius: 3px;
    padding: 0 3px;
  }
  .message {
    margin: 0 0 10px;
    font-size: 12px;
    color: #166534;
  }
  .message.error {
    color: #b91c1c;
  }
  .rows {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .row-card {
    background: white;
    border: 1px solid #e4e4e7;
    border-radius: 8px;
    padding: 10px 12px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .row-card.draft {
    margin-top: 10px;
    border-color: #2d7cf1;
    background: #f8fbff;
  }
  .row-grid {
    display: grid;
    grid-template-columns: 1fr 120px;
    gap: 8px;
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 3px;
    font-size: 11px;
    color: #52525b;
  }
  input,
  select,
  textarea {
    width: 100%;
    border: 1px solid #d4d4d8;
    border-radius: 4px;
    padding: 5px 8px;
    font-size: 12px;
    font-family: inherit;
    background: white;
    color: #18181b;
  }
  textarea {
    font-family: 'JetBrains Mono', monospace;
    font-size: 11px;
    resize: vertical;
  }
  input:focus,
  select:focus,
  textarea:focus {
    outline: none;
    border-color: #2d7cf1;
    box-shadow: 0 0 0 2px rgba(45, 124, 241, 0.2);
  }
  .row-actions {
    display: flex;
    align-items: center;
    gap: 8px;
    justify-content: flex-end;
  }
  .id-pill {
    margin-right: auto;
    font-family: 'JetBrains Mono', monospace;
    font-size: 10px;
    color: #71717a;
    background: #f4f4f5;
    border: 1px solid #e4e4e7;
    border-radius: 3px;
    padding: 1px 5px;
  }
  .btn {
    height: 28px;
    padding: 0 10px;
    background: white;
    border: 1px solid #d4d4d8;
    border-radius: 5px;
    font-size: 12px;
    font-family: inherit;
    cursor: pointer;
  }
  .btn:hover:not(:disabled) {
    border-color: #a1a1aa;
  }
  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .btn.primary {
    background: #2d7cf1;
    border-color: #2d7cf1;
    color: white;
  }
  .btn.danger:hover:not(:disabled) {
    background: #fee2e2;
    border-color: #ef4444;
    color: #b91c1c;
  }
  .footer {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 14px;
    padding-top: 12px;
    border-top: 1px solid #e4e4e7;
  }
</style>
