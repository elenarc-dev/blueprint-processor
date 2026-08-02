<!--
  Right side panel.
    * No selection → blueprint metadata editor + Blueprint Inputs editor.
    * Selection → node params editor.

  The selection is derived from the page's `flowNodes` mirror (a $state
  array that the page keeps in sync with the writable store via $effect).
  Reading from the mirror guarantees the Inspector reactively updates
  whenever Svelte Flow mutates the selection.

  Blueprint Inputs is ALWAYS visible at the bottom because it's the
  canonical declaration of every Blueprint Input node's output set
  (Blender group-input semantics). Editing the list here updates
  `meta.metadata.inputs` and is mirrored into the shared `blueprintInputs`
  module state by the page, so every Blueprint Input node's output
  handles automatically reflect the change.
-->
<script lang="ts">
  import type { BlueprintMeta, BlueprintInput } from '$lib/types/blueprint';
  import {
    getDef,
    NODE_CATALOG,
  } from '$lib/nodes/catalog';
  import {
    type FlowNode,
    type NodeData,
  } from '$lib/utils/blueprint-io';

  interface Props {
    meta: BlueprintMeta;
    nodes: FlowNode[];
    onMetaChange: (next: BlueprintMeta) => void;
    onNodeSave: (nodeId: string, patch: Partial<NodeData>) => void;
  }
  let { meta, nodes, onMetaChange, onNodeSave }: Props = $props();

  const selectedNode = $derived(
    nodes.find((n) => n.selected && (n.data as NodeData)?.kind) ?? null,
  );
  const def = $derived(
    selectedNode ? getDef((selectedNode.data as NodeData).kind) : undefined,
  );

  let open = $state(true);

  // The set of legal data types a BlueprintInput may carry. Three
  // picker-style kinds (`file`, `folder`, `multi-file`) hint at the
  // dialog UX in the dashboard/editor Run modal; the rest map to
  // their literal scalar/JSON branches. See `$lib/types/blueprint.ts`
  // `BlueprintInput.type`.
  const INPUT_TYPES = [
    'string',
    'number',
    'boolean',
    'file',
    'folder',
    'multi-file',
    'array',
    'any',
  ] as const;

  function commitMeta<K extends keyof BlueprintMeta>(
    key: K,
    value: BlueprintMeta[K],
  ) {
    onMetaChange({ ...meta, [key]: value, updated_at: new Date().toISOString() });
  }

  // ---------------------------------------------------------------------
  // Blueprint Inputs editing (always visible).
  //
  // `id` is READ-ONLY after creation. The id doubles as the runtime key
  // for `external_inputs` and as the source-port id of every Blueprint
  // Input node's output handle. Renaming mid-edit would orphan every
  // connected edge silently. The user can remove + add to rename. The
  // name (display label) stay free to edit any time.
  // ---------------------------------------------------------------------

  function commitInputs(next: BlueprintInput[]) {
    onMetaChange({
      ...meta,
      metadata: { ...meta.metadata, inputs: next },
      updated_at: new Date().toISOString(),
    });
  }

  function handleAddInput() {
    // Find the lowest `input_N` id that isn't taken.
    let i = 1;
    while (meta.metadata.inputs.some((inp) => inp.id === `input_${i}`)) i += 1;
    const fresh: BlueprintInput = {
      id: `input_${i}`,
      name: `Input ${i}`,
      type: 'string',
      default: undefined,
      description: '',
    };
    commitInputs([...meta.metadata.inputs, fresh]);
  }

  function handleRemoveInput(index: number) {
    const next = meta.metadata.inputs.filter((_, i) => i !== index);
    commitInputs(next);
  }

  function handleNameEdit(index: number, raw: string) {
    const next = meta.metadata.inputs.map((inp, i) =>
      i === index ? { ...inp, name: raw } : inp,
    );
    commitInputs(next);
  }

  function handleTypeEdit(index: number, raw: string) {
    const t = raw as BlueprintInput['type'];
    const next = meta.metadata.inputs.map((inp, i) =>
      i === index ? { ...inp, type: t } : inp,
    );
    commitInputs(next);
  }

  function handleDefaultEdit(index: number, raw: string) {
    // Parse JSON; on failure drop the default back to undefined so a
    // half-typed value doesn't poison the saved blueprint. We surface
    // a visual warning via the placeholder italic colour — the user
    // can re-type and the field will accept valid JSON again.
    let parsed: unknown;
    try {
      parsed = raw.trim() === '' ? undefined : JSON.parse(raw);
    } catch {
      parsed = undefined;
    }
    const next = meta.metadata.inputs.map((inp, i) =>
      i === index ? { ...inp, default: parsed } : inp,
    );
    commitInputs(next);
  }

  function handleDescriptionEdit(index: number, raw: string) {
    const next = meta.metadata.inputs.map((inp, i) =>
      i === index ? { ...inp, description: raw } : inp,
    );
    commitInputs(next);
  }
</script>

<aside class="inspector" class:collapsed={!open}>
  <header class="inspector-head">
    <button
      class="toggle"
      onclick={() => (open = !open)}
      title={open ? 'Collapse' : 'Expand'}
    >
      {open ? '▶' : '◀'}
    </button>
    {#if open}
      <span>{selectedNode ? def?.label ?? (selectedNode.data as NodeData).kind : 'Blueprint'}</span>
    {/if}
  </header>

  {#if open}
    <div class="body">
      {#if !selectedNode}
        <h4>Blueprint</h4>
        <div class="field">
          <span>Name</span>
          <input
            type="text"
            value={meta.name}
            oninput={(e) => commitMeta('name', e.currentTarget.value)}
          />
        </div>

        <div class="field">
          <span>Description</span>
          <textarea
            rows="4"
            value={meta.description}
            oninput={(e) => commitMeta('description', e.currentTarget.value)}
          ></textarea>
        </div>

        <div class="field">
          <span>Schema</span>
          <code>v{meta.schema_version}</code>
        </div>

        <div class="field">
          <span>ID</span>
          <code>{meta.id || '—'}</code>
        </div>

        <h4>Available kinds</h4>
        <ul class="kinds">
          {#each NODE_CATALOG as d (d.kind)}
            <li><code>{d.kind}</code></li>
          {/each}
        </ul>
      {:else if def}
        {@const d = selectedNode.data as NodeData}
        <h4>Node</h4>
        <div class="field">
          <span>ID</span>
          <code>{selectedNode.id}</code>
        </div>
        <div class="field">
          <span>Kind</span>
          <code>{d.kind}</code>
        </div>
        <div class="field">
          <span>Label (optional)</span>
          <input
            type="text"
            placeholder={def.label}
            value={d.label ?? ''}
            oninput={(e) =>
              onNodeSave(selectedNode.id, {
                label: e.currentTarget.value || undefined,
              })}
          />
        </div>

        {#if def.params.length > 0}
          <h4>Parameters</h4>
          {#each def.params as field (field.key)}
            <div class="field">
              <span>{field.label}</span>

              {#if field.kind === 'number'}
                <input
                  type="number"
                  step={field.step ?? 1}
                  min={field.min}
                  max={field.max}
                  value={(d.params[field.key] as number | undefined) ?? 0}
                  oninput={(e) =>
                    onNodeSave(selectedNode.id, {
                      params: {
                        ...d.params,
                        [field.key]: Number(e.currentTarget.value),
                      },
                    })}
                />
              {:else if field.kind === 'string'}
                <input
                  type="text"
                  placeholder={field.placeholder ?? ''}
                  value={(d.params[field.key] as string | undefined) ?? ''}
                  oninput={(e) =>
                    onNodeSave(selectedNode.id, {
                      params: {
                        ...d.params,
                        [field.key]: e.currentTarget.value,
                      },
                    })}
                />
              {:else if field.kind === 'boolean'}
                <label class="check">
                  <input
                    type="checkbox"
                    checked={!!d.params[field.key]}
                    onchange={(e) =>
                      onNodeSave(selectedNode.id, {
                        params: {
                          ...d.params,
                          [field.key]: e.currentTarget.checked,
                        },
                      })}
                  />
                  <span>
                    {d.params[field.key] ? 'true' : 'false'}
                  </span>
                </label>
              {:else if field.kind === 'select'}
                <select
                  value={(d.params[field.key] as string | undefined) ?? field.default}
                  onchange={(e) =>
                    onNodeSave(selectedNode.id, {
                      params: {
                        ...d.params,
                        [field.key]: e.currentTarget.value,
                      },
                    })}
                >
                  {#each field.options as opt (opt)}
                    <option value={opt}>{opt}</option>
                  {/each}
                </select>
              {/if}
            </div>
          {/each}
        {:else}
          <p class="muted">This node has no editable parameters.</p>
        {/if}
      {:else}
        <p class="muted">Unknown node kind.</p>
      {/if}

      <h4>Blueprint Inputs</h4>
      <p class="muted small">
        Declared inputs appear as output ports on every
        <code>Blueprint Input</code> node in the graph.
      </p>
      {#if meta.metadata.inputs.length === 0}
        <div class="empty-inputs">
          <span class="muted">No inputs declared yet.</span>
        </div>
      {:else}
        <ul class="inputs-list">
          {#each meta.metadata.inputs as input, i (input.id)}
            <li class="input-row">
              <div class="input-row-line">
                <input
                  type="text"
                  class="id-input"
                  value={input.id}
                  placeholder="input_id"
                  title="Runtime key — read-only; remove + add to rename"
                  readonly
                  aria-readonly="true"
                />
                <input
                  type="text"
                  class="name-input"
                  value={input.name}
                  placeholder="Display name"
                  oninput={(e) => handleNameEdit(i, e.currentTarget.value)}
                />
                <select
                  class="type-select"
                  value={input.type}
                  onchange={(e) => handleTypeEdit(i, e.currentTarget.value)}
                >
                  {#each INPUT_TYPES as t (t)}
                    <option value={t}>{t}</option>
                  {/each}
                </select>
                <button
                  class="remove-btn"
                  type="button"
                  title="Remove this input"
                  aria-label="Remove input"
                  onclick={() => handleRemoveInput(i)}
                >×</button>
              </div>
              <div class="input-row-line sub">
                <span class="sub-label">default</span>
                <input
                  type="text"
                  class="default-input"
                  spellcheck="false"
                  value={input.default === undefined
                    ? ''
                    : (() => {
                        try {
                          return JSON.stringify(input.default);
                        } catch {
                          return '';
                        }
                      })()}
                  placeholder="(empty — JSON)"
                  oninput={(e) => handleDefaultEdit(i, e.currentTarget.value)}
                />
              </div>
              <div class="input-row-line sub">
                <span class="sub-label">description</span>
                <input
                  type="text"
                  class="desc-input"
                  value={input.description ?? ''}
                  placeholder="(none)"
                  oninput={(e) => handleDescriptionEdit(i, e.currentTarget.value)}
                />
              </div>
            </li>
          {/each}
        </ul>
      {/if}
      <div class="add-row">
        <button class="add-btn" type="button" onclick={handleAddInput}>
          + Add input
        </button>
      </div>
    </div>
  {/if}
</aside>

<style>
  .inspector {
    width: 320px;
    flex-shrink: 0;
    background: #fafafa;
    border-left: 1px solid #d4d4d8;
    display: flex;
    flex-direction: column;
    font-family: 'Inter', system-ui, sans-serif;
    transition: width 0.18s ease;
  }
  .inspector.collapsed {
    width: 32px;
  }

  .inspector-head {
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

  .body {
    flex: 1;
    overflow-y: auto;
    padding: 10px 14px 32px;
  }

  h4 {
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: #52525b;
    margin: 12px 0 6px;
  }

  .field {
    display: block;
    margin-bottom: 10px;
  }
  .field > span {
    display: block;
    font-size: 11px;
    color: #52525b;
    margin-bottom: 3px;
  }
  .field code {
    font-family: 'JetBrains Mono', monospace;
    font-size: 11px;
    color: #3f3f46;
    background: #f4f4f5;
    padding: 1px 5px;
    border-radius: 3px;
    border: 1px solid #e4e4e7;
    display: inline-block;
  }
  .field input[type='text'],
  .field textarea,
  .field select,
  .field input[type='number'] {
    width: 100%;
    border: 1px solid #d4d4d8;
    border-radius: 4px;
    padding: 5px 8px;
    font-size: 12.5px;
    background: white;
    color: #18181b;
    font-family: inherit;
  }
  .field input:focus,
  .field textarea:focus,
  .field select:focus {
    outline: none;
    border-color: #2d7cf1;
    box-shadow: 0 0 0 2px rgba(45, 124, 241, 0.2);
  }

  .check {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 12.5px;
    color: #3f3f46;
  }

  .muted {
    color: #71717a;
    font-size: 12px;
    font-style: italic;
    margin: 12px 0;
  }
  .small {
    font-size: 11px;
    margin: 0 0 6px;
  }
  .muted code {
    font-family: 'JetBrains Mono', monospace;
    background: #f4f4f5;
    border: 1px solid #e4e4e7;
    padding: 1px 4px;
    border-radius: 3px;
    font-size: 10.5px;
  }

  .kinds {
    list-style: none;
    padding: 0;
    margin: 0;
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }
  .kinds li code {
    font-family: 'JetBrains Mono', monospace;
    font-size: 10.5px;
    background: #f4f4f5;
    padding: 1px 5px;
    border-radius: 3px;
    border: 1px solid #e4e4e7;
    color: #52525b;
  }

  /* Blueprint Inputs editor block. Always visible. */
  .empty-inputs {
    background: #f4f4f5;
    border: 1px dashed #d4d4d8;
    border-radius: 4px;
    padding: 8px 10px;
    margin-bottom: 8px;
  }
  .inputs-list {
    list-style: none;
    padding: 0;
    margin: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .input-row {
    background: white;
    border: 1px solid #d4d4d8;
    border-radius: 5px;
    padding: 6px 8px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .input-row-line {
    display: flex;
    gap: 6px;
    align-items: center;
  }
  .input-row-line.sub {
    gap: 4px;
    align-items: baseline;
  }
  .sub-label {
    font-family: 'JetBrains Mono', monospace;
    font-size: 10px;
    background: #f4f4f5;
    border: 1px solid #e4e4e7;
    border-radius: 3px;
    padding: 1px 5px;
    color: #52525b;
    flex: 0 0 auto;
  }
  .id-input {
    flex: 0 0 110px;
    font-family: 'JetBrains Mono', monospace;
    font-size: 11px;
    background: #f4f4f5;
    border: 1px solid #e4e4e7;
    border-radius: 3px;
    padding: 3px 6px;
    color: #3f3f46;
  }
  .name-input {
    flex: 1 1 0;
    min-width: 0;
    border: 1px solid #d4d4d8;
    border-radius: 3px;
    padding: 3px 6px;
    font-size: 11.5px;
  }
  .type-select {
    /* Bumped from 80px to fit 'multi-file' (longest value in
       INPUT_TYPES) without truncation; the row flex layout still
       accommodates the wider select. */
    flex: 0 0 110px;
    border: 1px solid #d4d4d8;
    border-radius: 3px;
    padding: 3px 6px;
    font-size: 11.5px;
    font-family: 'JetBrains Mono', monospace;
  }
  .remove-btn {
    flex: 0 0 22px;
    width: 22px;
    height: 22px;
    font-size: 14px;
    line-height: 1;
    border: 1px solid #e4e4e7;
    background: white;
    border-radius: 3px;
    color: #6b7280;
    cursor: pointer;
    padding: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    transition:
      background 0.1s ease,
      border-color 0.1s ease,
      color 0.1s ease;
  }
  .remove-btn:hover {
    background: #fee2e2;
    border-color: #ef4444;
    color: #ef4444;
  }
  .default-input,
  .desc-input {
    flex: 1 1 0;
    min-width: 0;
    border: 1px solid #d4d4d8;
    border-radius: 3px;
    padding: 2px 6px;
    font-size: 11px;
    font-family: 'JetBrains Mono', monospace;
  }
  .default-input {
    background: #fafafa;
  }
  .default-input:focus,
  .desc-input:focus {
    outline: none;
    border-color: #2d7cf1;
    box-shadow: 0 0 0 2px rgba(45, 124, 241, 0.2);
  }
  .add-row {
    display: flex;
    justify-content: center;
    margin-top: 8px;
  }
  .add-btn {
    background: white;
    border: 1px solid #2d7cf1;
    border-radius: 4px;
    padding: 5px 14px;
    font-size: 12px;
    font-weight: 500;
    color: #2d7cf1;
    cursor: pointer;
    font-family: inherit;
    transition:
      background 0.1s ease,
      color 0.1s ease,
      transform 0.05s ease;
  }
  .add-btn:hover {
    background: #2d7cf1;
    color: white;
  }
  .add-btn:active {
    transform: scale(0.97);
  }
</style>
