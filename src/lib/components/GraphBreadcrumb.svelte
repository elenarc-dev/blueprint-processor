<!--
  Breadcrumb for nested graph editing.

  Opening a `blueprint.subgraph` / `ai.tool` file swaps the canvas to the
  embedded blueprint while the parent stays in the page's `graphStack`.
  This bar makes that nesting visible and is the way back: every ancestor
  is clickable, and the trailing button returns one level.
-->
<script lang="ts">
  interface TrailCrumb {
    name: string;
    /** Index in the page's graph stack; the live graph is `stack.length`. */
    depth: number;
  }

  interface Props {
    trail: TrailCrumb[];
    onNavigate: (depth: number) => void;
    onBack: () => void;
  }
  let { trail, onNavigate, onBack }: Props = $props();
</script>

{#if trail.length > 1}
  <nav class="nest-bar" aria-label="Blueprint nesting">
    <span class="label">Nested</span>
    <div class="crumbs">
      {#each trail as crumb, i (crumb.depth)}
        {#if i > 0}
          <span class="sep" aria-hidden="true">›</span>
        {/if}
        {#if i === trail.length - 1}
          <span class="crumb current" title="Current graph">{crumb.name}</span>
        {:else}
          <button
            type="button"
            class="crumb"
            title="Back to {crumb.name}"
            onclick={() => onNavigate(crumb.depth)}
          >{crumb.name}</button>
        {/if}
      {/each}
    </div>
    <span class="spacer"></span>
    <button type="button" class="back" onclick={onBack}>
      ← Parent graph
    </button>
  </nav>
{/if}

<style>
  .nest-bar {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    height: 34px;
    padding: 0 12px;
    background: #eef2ff;
    border-bottom: 1px solid #c7d2fe;
    font-family: 'Inter', system-ui, sans-serif;
    font-size: 12px;
    color: #3730a3;
  }
  .label {
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: #6366f1;
    flex: 0 0 auto;
  }
  .crumbs {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    overflow-x: auto;
    padding-bottom: 1px;
  }
  .sep {
    color: #a5b4fc;
    flex: 0 0 auto;
  }
  .crumb {
    border: none;
    background: transparent;
    color: #4338ca;
    font-family: inherit;
    font-size: 12px;
    padding: 2px 4px;
    border-radius: 3px;
    cursor: pointer;
    white-space: nowrap;
    max-width: 220px;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .crumb:hover {
    background: #e0e7ff;
    text-decoration: underline;
  }
  .crumb.current {
    color: #1e1b4b;
    font-weight: 600;
    cursor: default;
    max-width: 260px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .spacer {
    flex: 1 1 0;
  }
  .back {
    flex: 0 0 auto;
    border: 1px solid #6366f1;
    background: white;
    color: #4338ca;
    border-radius: 5px;
    height: 24px;
    padding: 0 10px;
    font-family: inherit;
    font-size: 11.5px;
    font-weight: 600;
    cursor: pointer;
    transition:
      background 0.1s ease,
      color 0.1s ease,
      transform 0.05s ease;
  }
  .back:hover {
    background: #6366f1;
    color: white;
  }
  .back:active {
    transform: scale(0.97);
  }
</style>
