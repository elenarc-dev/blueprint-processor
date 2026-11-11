<!--
  Right-hand drawer with every log entry of the last run.

  Two things this fixes over the bottom drawer's log column:
    * entries survive a FAILED run (Rust now returns them with the
      error instead of throwing them away), and
    * levels are first-class: filter by severity or search across
      label / kind / node id / value, then copy the whole dump.
-->
<script lang="ts">
  import type { LogLevel, RunLog, RunResult } from '$lib/utils/runner-io';
  import { renderValue } from '$lib/utils/runner-io';

  interface Props {
    result: RunResult | null;
    open: boolean;
    onClose: () => void;
  }
  let { result, open, onClose }: Props = $props();

  type LevelFilter = 'all' | LogLevel;
  const LEVELS: LevelFilter[] = ['all', 'debug', 'info', 'warn', 'error'];

  let level = $state<LevelFilter>('all');
  let query = $state('');
  /** Indices (into the unfiltered list) of entries expanded to full JSON. */
  let expanded = $state<number[]>([]);
  let copied = $state(false);

  const logs = $derived<RunLog[]>(result?.logs ?? []);

  const counts = $derived.by(() => {
    const c: Record<LevelFilter, number> = {
      all: logs.length,
      debug: 0,
      info: 0,
      warn: 0,
      error: 0,
    };
    for (const l of logs) c[l.level] += 1;
    return c;
  });

  const visible = $derived.by(() => {
    const q = query.trim().toLowerCase();
    return logs
      .map((log, i) => ({ log, i }))
      .filter(({ log }) => {
        if (level !== 'all' && log.level !== level) return false;
        if (!q) return true;
        return (
          log.label.toLowerCase().includes(q) ||
          log.kind.toLowerCase().includes(q) ||
          log.node_id.toLowerCase().includes(q) ||
          renderValue(log.value).toLowerCase().includes(q)
        );
      });
  });

  function toggle(i: number) {
    expanded = expanded.includes(i)
      ? expanded.filter((x) => x !== i)
      : [...expanded, i];
  }

  /** Plain-text dump for pasting into an issue or a chat. */
  function dump(): string {
    const head = result
      ? `# Run ${result.ok ? 'ok' : 'failed'} · ${result.stats.nodes_executed} nodes · ${result.stats.duration_ms} ms`
      : '# No run yet';
    const lines = logs.map(
      (l, i) =>
        `[${i + 1}] ${l.level.toUpperCase().padEnd(5)} ${l.kind} (${l.node_id || 'run'}) ${l.label}: ${JSON.stringify(l.value)}`,
    );
    return [
      head,
      result?.error ? `# error: ${result.error}` : '',
      ...lines,
    ]
      .filter(Boolean)
      .join('\n');
  }

  async function copyAll() {
    const text = dump();
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      // Webview without clipboard permission: fall back to the old
      // selection trick so Copy never silently does nothing.
      const ta = document.createElement('textarea');
      ta.value = text;
      document.body.appendChild(ta);
      ta.select();
      document.execCommand('copy');
      ta.remove();
    }
    copied = true;
    setTimeout(() => (copied = false), 1200);
  }
</script>

<aside
  class="logs-panel"
  class:collapsed={!open}
  class:failed={!!result && !result.ok}
  aria-label="Run logs"
>
  <header class="panel-head">
    <span class="title">Logs</span>
    {#if result}
      <span class="count">{logs.length}</span>
    {/if}
    <span class="spacer"></span>
    {#if logs.length > 0}
      <button class="icon-btn" onclick={copyAll} title="Copy all entries as text">
        {copied ? 'Copied' : 'Copy'}
      </button>
    {/if}
    <button class="icon-btn" onclick={onClose} title="Close log panel">×</button>
  </header>

  {#if open}
    {#if !result}
      <p class="muted">Run the blueprint to see its logs here.</p>
    {:else}
      <div class="run-meta">
        <span class="badge" class:ok={result.ok} class:err={!result.ok}>
          {result.ok ? 'ok' : 'failed'}
        </span>
        <span class="muted">
          {result.stats.nodes_executed} nodes · {result.stats.duration_ms} ms
        </span>
      </div>

      {#if result.error}
        <pre class="error">{result.error}</pre>
      {/if}

      <div class="filters">
        {#each LEVELS as lv (lv)}
          <button
            class="chip"
            class:active={level === lv}
            class:empty={lv !== 'all' && counts[lv] === 0}
            onclick={() => (level = lv)}
          >
            {lv}{#if lv !== 'all' && counts[lv] > 0}<span class="n">{counts[lv]}</span>{/if}
          </button>
        {/each}
      </div>

      <input
        class="search"
        type="search"
        placeholder="Filter by label, kind, node or value…"
        bind:value={query}
      />

      <div class="entries">
        {#if visible.length === 0}
          <p class="muted">
            {logs.length === 0
              ? 'No log entries this run.'
              : 'No entries match the filter.'}
          </p>
        {:else}
          <ol>
            {#each visible as { log, i } (i)}
              <li class="entry lv-{log.level}">
                <button
                  class="row"
                  onclick={() => toggle(i)}
                  title={expanded.includes(i) ? 'Collapse value' : 'Expand full value'}
                >
                  <span class="dot"></span>
                  <span class="label">{log.label}</span>
                  <span class="kind">{log.kind}</span>
                  <span class="node">{log.node_id || 'run'}</span>
                </button>
                {#if expanded.includes(i)}
                  <pre class="value full">{JSON.stringify(log.value, null, 2)}</pre>
                {:else}
                  <code class="value">{renderValue(log.value)}</code>
                {/if}
              </li>
            {/each}
          </ol>
        {/if}
      </div>
    {/if}
  {/if}
</aside>

<style>
  .logs-panel {
    width: 340px;
    flex-shrink: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    background: #fafafa;
    border-left: 1px solid #d4d4d8;
    font-family: 'Inter', system-ui, sans-serif;
    transition: width 0.14s ease;
  }
  .logs-panel.collapsed {
    width: 0;
    border-left: none;
    overflow: hidden;
  }
  .logs-panel.failed {
    border-left-color: #fca5a5;
  }

  .panel-head {
    height: 36px;
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 10px;
    background: #e4e4e7;
    border-bottom: 1px solid #d4d4d8;
    font-size: 12px;
    font-weight: 600;
    color: #3f3f46;
    white-space: nowrap;
  }
  .title {
    font-weight: 700;
  }
  .count {
    background: #3f3f46;
    color: #e4e4e7;
    border-radius: 8px;
    padding: 0 6px;
    font-size: 10.5px;
    font-family: 'JetBrains Mono', monospace;
  }
  .spacer {
    flex: 1;
  }
  .icon-btn {
    background: transparent;
    border: 1px solid transparent;
    border-radius: 4px;
    color: #52525b;
    font-size: 11px;
    cursor: pointer;
    padding: 2px 6px;
  }
  .icon-btn:hover {
    background: #d4d4d8;
  }

  .run-meta {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 10px 0;
  }
  .badge {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    padding: 1px 7px;
    border-radius: 3px;
    font-family: 'JetBrains Mono', monospace;
    font-weight: 700;
  }
  .badge.ok {
    background: #16a34a;
    color: white;
  }
  .badge.err {
    background: #dc2626;
    color: white;
  }

  .error {
    margin: 8px 10px 0;
    padding: 6px 8px;
    background: #fef2f2;
    border: 1px solid #fecaca;
    border-radius: 4px;
    color: #991b1b;
    font-family: 'JetBrains Mono', monospace;
    font-size: 11px;
    white-space: pre-wrap;
    word-break: break-word;
    max-height: 120px;
    overflow: auto;
  }

  .filters {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    padding: 8px 10px 0;
  }
  .chip {
    border: 1px solid #d4d4d8;
    background: white;
    border-radius: 10px;
    font-size: 10.5px;
    padding: 1px 8px;
    color: #3f3f46;
    cursor: pointer;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .chip.active {
    background: #18181b;
    border-color: #18181b;
    color: #fafafa;
  }
  .chip.empty {
    opacity: 0.45;
  }
  .chip .n {
    margin-left: 4px;
    font-family: 'JetBrains Mono', monospace;
  }

  .search {
    margin: 8px 10px;
    height: 24px;
    padding: 0 8px;
    border: 1px solid #d4d4d8;
    border-radius: 4px;
    font-size: 11.5px;
    background: white;
  }
  .search:focus {
    outline: none;
    border-color: #a1a1aa;
  }

  .entries {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 0 10px 12px;
  }
  .entries ol {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .entry {
    border-bottom: 1px dashed #e4e4e7;
    padding: 3px 0 4px;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    background: transparent;
    border: none;
    padding: 1px 0;
    cursor: pointer;
    text-align: left;
    font-size: 11.5px;
    color: #18181b;
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    flex-shrink: 0;
    background: #94a3b8;
  }
  .lv-info .dot {
    background: #2563eb;
  }
  .lv-warn .dot {
    background: #d97706;
  }
  .lv-error .dot {
    background: #dc2626;
  }
  .label {
    font-weight: 600;
    font-family: 'JetBrains Mono', monospace;
  }
  .kind,
  .node {
    font-family: 'JetBrains Mono', monospace;
    font-size: 10px;
    color: #71717a;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .kind {
    max-width: 96px;
  }
  .node {
    margin-left: auto;
    max-width: 72px;
  }
  .lv-warn .label {
    color: #b45309;
  }
  .lv-error .label {
    color: #b91c1c;
  }

  .value {
    display: block;
    margin: 2px 0 0 13px;
    font-family: 'JetBrains Mono', monospace;
    font-size: 11px;
    color: #1e3a8a;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .value.full {
    white-space: pre-wrap;
    word-break: break-word;
    max-height: 260px;
    overflow: auto;
    background: #f4f4f5;
    border: 1px solid #e4e4e7;
    border-radius: 3px;
    padding: 4px 6px;
  }

  .muted {
    color: #71717a;
    font-size: 11.5px;
    margin: 8px 10px;
  }
</style>
