<!--
  Bottom drawer that shows the result of the last blueprint execution.
  Layout:
    [Header] Run summary  ·  Run button  ·  Clear button
    [Body]   outputs | logs (split)
-->
<script lang="ts">
  import type { RunResult } from '$lib/utils/runner-io';
  import { renderValue } from '$lib/utils/runner-io';

  interface Props {
    result: RunResult | null;
    running: boolean;
    onRun: () => void;
    onClear: () => void;
    /** Opens the full log drawer on the right. */
    onOpenLogs: () => void;
  }
  let { result, running, onRun, onClear, onOpenLogs }: Props = $props();

  let open = $state(true);
</script>

<section class="results" class:collapsed={!open}>
  <header class="results-head">
    <button class="toggle" onclick={() => (open = !open)} title={open ? 'Collapse' : 'Expand'}>
      {open ? '▼' : '▲'}
    </button>
    <span class="title">Run</span>

    {#if result}
      <span class="badge" class:ok={result.ok} class:err={!result.ok}>
        {result.ok ? 'ok' : 'failed'} · {result.stats.nodes_executed} nodes · {result.stats.duration_ms} ms
      </span>
    {/if}

    <button class="btn btn-primary" onclick={onRun} disabled={running}>
      {running ? 'Running…' : '▶ Run'}
    </button>
    {#if result}
      <button class="btn" onclick={onClear}>Clear</button>
    {/if}
  </header>

  {#if open}
    <div class="results-body">
      {#if !result}
        <p class="muted">Press Run to execute the current blueprint.</p>
      {:else}
        <div class="split">
          <div class="col">
            <h4>Outputs ({Object.keys(result.outputs).length})</h4>
            {#if Object.keys(result.outputs).length === 0}
              <p class="muted">No <code>io.output</code> nodes reached.</p>
            {:else}
              <ul class="kv">
                {#each Object.entries(result.outputs) as [name, value] (name)}
                  <li>
                    <span class="key">{name}</span>
                    <span class="val">{renderValue(value)}</span>
                  </li>
                {/each}
              </ul>
            {/if}
          </div>
          <div class="col">
            <h4>Logs ({result.logs.length})</h4>
            {#if result.logs.length === 0}
              <p class="muted">No <code>io.logger</code> nodes reached.</p>
            {:else}
              <p class="muted">
                {result.logs.filter((l) => l.level === 'warn').length} warnings ·
                {result.logs.filter((l) => l.level === 'error').length} errors
              </p>
              <button class="btn" onclick={onOpenLogs}>Open log panel →</button>
            {/if}
          </div>
        </div>
      {/if}
    </div>
  {/if}
</section>

<style>
  .results {
    flex-shrink: 0;
    background: #fafafa;
    border-top: 1px solid #d4d4d8;
    font-family: 'Inter', system-ui, sans-serif;
    display: flex;
    flex-direction: column;
  }
  .results-head {
    height: 36px;
    background: #e4e4e7;
    border-bottom: 1px solid #d4d4d8;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 10px;
    font-weight: 600;
    font-size: 12px;
    color: #3f3f46;
  }
  .toggle {
    background: transparent;
    border: none;
    color: #52525b;
    cursor: pointer;
    padding: 2px 6px;
    font-size: 11px;
    border-radius: 3px;
  }
  .toggle:hover {
    background: #d4d4d8;
  }
  .title {
    font-weight: 700;
  }
  .badge {
    background: #3f3f46;
    color: #d4d4d8;
    font-size: 10.5px;
    padding: 1px 8px;
    border-radius: 3px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    font-family: 'JetBrains Mono', monospace;
  }
  .badge.ok {
    background: #16a34a;
    color: white;
  }
  .badge.err {
    background: #dc2626;
    color: white;
  }

  .btn {
    height: 26px;
    padding: 0 10px;
    background: white;
    border: 1px solid #d4d4d8;
    border-radius: 4px;
    color: #27272a;
    font-size: 11.5px;
    font-weight: 500;
    cursor: pointer;
  }
  .btn:hover:not(:disabled) {
    background: #f4f4f5;
    border-color: #a1a1aa;
  }
  .btn-primary {
    background: #16a34a;
    border-color: #16a34a;
    color: white;
  }
  .btn-primary:hover:not(:disabled) {
    background: #15803d;
  }
  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .results-body {
    padding: 8px 14px 12px;
    max-height: 220px;
    overflow-y: auto;
  }

  .muted {
    color: #71717a;
    font-size: 12px;
    margin: 6px 0;
  }
  .muted code {
    font-family: 'JetBrains Mono', monospace;
    background: #f4f4f5;
    padding: 1px 5px;
    border-radius: 3px;
    border: 1px solid #e4e4e7;
  }

  .split {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 18px;
  }
  h4 {
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: #52525b;
    margin: 4px 0 6px;
  }

  .kv {
    list-style: none;
    padding: 0;
    margin: 0;
  }
  .kv li {
    display: flex;
    align-items: baseline;
    gap: 8px;
    padding: 3px 0;
    border-bottom: 1px dashed #e4e4e7;
    font-size: 12px;
  }
  .key {
    font-family: 'JetBrains Mono', monospace;
    font-weight: 600;
    color: #0f172a;
    min-width: 80px;
  }
  .val {
    font-family: 'JetBrains Mono', monospace;
    color: #1e3a8a;
    word-break: break-all;
  }

</style>
