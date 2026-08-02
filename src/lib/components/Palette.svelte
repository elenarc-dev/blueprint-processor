<!--
  Left sidebar palette. Lists every node kind grouped by category.
  Each entry is `draggable` so the user can drop into the canvas.

  Drag payload is JSON-encoded with both `kind` and `position` (the
  page drop event). The Editor reads it via `application/x-blueprint-node`.
-->
<script lang="ts">
  import { NODE_CATALOG, CATEGORIES, CATEGORY_COLORS, type NodeCategory } from '$lib/nodes/catalog';

  const grouped = $derived(
    CATEGORIES.map((cat) => ({
      cat,
      items: NODE_CATALOG.filter((d) => d.category === cat),
    })),
  );

  function onDragStart(event: DragEvent, kind: string) {
    if (!event.dataTransfer) return;
    event.dataTransfer.setData(
      'application/x-blueprint-node',
      JSON.stringify({ kind }),
    );
    event.dataTransfer.effectAllowed = 'copy';
  }

  // Click-to-spawn fallback: when the user clicks (instead of drags) a
  // palette item, broadcast a window event so the canvas can place the
  // node at its centre. Works even if the WebView refuses HTML5 D&D.
  function onItemClick(kind: string) {
    window.dispatchEvent(
      new CustomEvent('blueprint:add-node', { detail: { kind } }),
    );
  }

  let open = $state(true);
</script>

<aside class="palette" class:collapsed={!open}>
  <header class="palette-head">
    <button class="toggle" onclick={() => (open = !open)} title={open ? 'Collapse' : 'Expand'}>
      {open ? '◀' : '▶'}
    </button>
    {#if open}
      <span>Node Palette</span>
    {/if}
  </header>

  {#if open}
    <div class="palette-body">
      {#each grouped as group (group.cat)}
        <section class="category">
          <h4 style="--cat-color: {CATEGORY_COLORS[group.cat as NodeCategory]}">
            {group.cat}
          </h4>
          <ul>
            <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
            {#each group.items as def (def.kind)}
              <li
                draggable="true"
                ondragstart={(e) => onDragStart(e, def.kind)}
                onclick={() => onItemClick(def.kind)}
                title={def.description}
              >
                <span class="dot" style="background: {CATEGORY_COLORS[group.cat as NodeCategory]}"></span>
                {def.label}
              </li>
            {/each}
          </ul>
        </section>
      {/each}
    </div>
  {/if}
</aside>

<style>
  .palette {
    width: 240px;
    flex-shrink: 0;
    background: #f4f4f5;
    border-right: 1px solid #d4d4d8;
    display: flex;
    flex-direction: column;
    font-family: 'Inter', system-ui, sans-serif;
    transition: width 0.18s ease;
  }
  .palette.collapsed {
    width: 32px;
  }

  .palette-head {
    height: 38px;
    background: #e4e4e7;
    border-bottom: 1px solid #d4d4d8;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 8px;
    font-weight: 600;
    font-size: 13px;
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

  .palette-body {
    overflow-y: auto;
    flex: 1;
    padding: 8px 0 24px;
  }

  .category {
    padding: 4px 8px 10px;
  }
  .category h4 {
    margin: 6px 4px 6px;
    font-size: 11px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--cat-color, #52525b);
    font-weight: 700;
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  li {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 8px;
    margin: 2px 0;
    border-radius: 4px;
    cursor: grab;
    background: white;
    border: 1px solid #e4e4e7;
    color: #27272a;
    font-size: 12.5px;
    user-select: none;
    transition: background 0.1s ease, border-color 0.1s ease, transform 0.05s ease;
  }
  li:hover {
    background: #fafafa;
    border-color: #2d7cf1;
  }
  li:active {
    cursor: grabbing;
    transform: scale(0.98);
  }

  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex-shrink: 0;
    box-shadow: 0 0 0 1px rgba(0, 0, 0, 0.08);
  }
</style>
