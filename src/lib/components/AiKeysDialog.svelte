<!--
  AI provider API keys.

  Keys are stored by the Rust side in `<app_data_dir>/secrets.json` with
  owner-only permissions (see src-tauri/src/ai/secrets.rs). They are
  deliberately NOT editable from a node: anything in a node's `params`
  is written into the `.blueprint.json`, which users save, diff and
  share. This dialog is the only place a key ever enters the app.

  The backend never returns a key — `ai_key_status` reports
  `{ provider, configured, hint }` only, so the full secret never comes
  back over the IPC bridge.
-->
<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';

  interface Props {
    open: boolean;
  }
  let { open = $bindable(false) }: Props = $props();

  interface SecretStatus {
    provider: string;
    configured: boolean;
    hint: string | null;
  }

  /** Human label per provider id. Unknown ids fall back to the raw id. */
  const PROVIDER_LABELS: Record<string, string> = {
    openai: 'OpenAI-compatible (OpenAI, Groq, OpenRouter, Ollama…)',
    anthropic: 'Anthropic',
  };

  let statuses = $state<SecretStatus[]>([]);
  let drafts = $state<Record<string, string>>({});
  let busy = $state<string | null>(null);
  let message = $state<{ text: string; kind: 'info' | 'error' } | null>(null);

  // Re-read status every time the dialog opens so a key saved in a
  // previous session (or removed by hand) shows the real state.
  $effect(() => {
    if (open) void refresh();
  });

  async function refresh() {
    try {
      statuses = await invoke<SecretStatus[]>('ai_key_status');
    } catch (err) {
      message = { text: `Could not read key status: ${err}`, kind: 'error' };
    }
  }

  async function save(provider: string) {
    const key = (drafts[provider] ?? '').trim();
    if (!key) {
      message = { text: 'Paste a key first.', kind: 'error' };
      return;
    }
    busy = provider;
    try {
      statuses = await invoke<SecretStatus[]>('ai_set_key', { provider, key });
      drafts = { ...drafts, [provider]: '' };
      message = { text: `Key saved for ${provider}.`, kind: 'info' };
    } catch (err) {
      message = { text: String(err), kind: 'error' };
    } finally {
      busy = null;
    }
  }

  async function clear(provider: string) {
    busy = provider;
    try {
      statuses = await invoke<SecretStatus[]>('ai_clear_key', { provider });
      message = { text: `Removed the ${provider} key.`, kind: 'info' };
    } catch (err) {
      message = { text: String(err), kind: 'error' };
    } finally {
      busy = null;
    }
  }

  function close() {
    open = false;
    message = null;
  }
</script>

{#if open}
  <div class="backdrop"></div>
  <div class="panel">
    <h3>AI provider keys</h3>
    <p class="muted">
      Stored on this machine only, in the app data directory with
      owner-only permissions. Keys are never written into a blueprint —
      a blueprint that uses an AI node stays safe to share.
    </p>

    {#each statuses as status (status.provider)}
      <div class="provider">
        <div class="provider-head">
          <strong>{PROVIDER_LABELS[status.provider] ?? status.provider}</strong>
          {#if status.configured}
            <span class="pill ok">configured {status.hint ?? ''}</span>
          {:else}
            <span class="pill">not configured</span>
          {/if}
        </div>
        <div class="provider-row">
          <label class="sr-only" for={`key-${status.provider}`}>
            API key for {status.provider}
          </label>
          <input
            id={`key-${status.provider}`}
            type="password"
            placeholder="Paste API key…"
            autocomplete="off"
            spellcheck="false"
            bind:value={drafts[status.provider]}
          />
          <button
            class="btn"
            onclick={() => save(status.provider)}
            disabled={busy === status.provider}
          >
            {busy === status.provider ? 'Saving…' : 'Save'}
          </button>
          <button
            class="btn danger"
            onclick={() => clear(status.provider)}
            disabled={!status.configured || busy === status.provider}
          >
            Clear
          </button>
        </div>
      </div>
    {/each}

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
    width: min(520px, calc(100vw - 32px));
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
  .provider {
    border-top: 1px solid #f0f0f3;
    padding: 12px 0;
  }
  .provider-head {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 6px;
    font-size: 12.5px;
  }
  .provider-row {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .provider-row input {
    flex: 1;
    min-width: 0;
    height: 28px;
    border: 1px solid #d4d4d8;
    border-radius: 4px;
    padding: 0 8px;
    font-size: 12px;
    font-family: inherit;
  }
  .provider-row input:focus {
    outline: none;
    border-color: #2d7cf1;
    box-shadow: 0 0 0 2px rgba(45, 124, 241, 0.2);
  }
  .pill {
    font-size: 10.5px;
    padding: 1px 6px;
    border-radius: 999px;
    background: #f4f4f5;
    color: #52525b;
    border: 1px solid #e4e4e7;
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
  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip-path: inset(50%);
    white-space: nowrap;
  }
</style>
