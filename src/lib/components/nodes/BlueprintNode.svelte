<!--
  Generic Svelte Flow node renderer for ALL blueprint node kinds.
  The visual structure (header, body, handles) is identical for every
  node; only the inline param editor in the middle changes per `data.kind`.
  Inline edits push changes back via Svelte Flow's own updateNodeData,
  which propagates through the writable nodes store the host owns.

  PARSER NOTE for future contributors:
    Inside HTML element attribute lists, NEVER place a Svelte-style
    comment `{/** */}` between attributes. Svelte 5 misreads the
    leading curly as the start of an attribute shorthand expression
    and reports "Expected token =" at the comment site. Use plain
    HTML `\<\!--  \--\>` comments (with no backticks or apostrophes
    inside element-attribute zones) or move the comment out of the
    attribute list entirely.
-->
<script lang="ts">
  import {
    Handle,
    Position,
    useSvelteFlow,
    type NodeProps,
  } from '@xyflow/svelte';
  import { type NodeData, portFromBlueprintInput } from '$lib/utils/blueprint-io';
  import {
    getDef,
    CATEGORY_COLORS,
    resolvedRepeatCount,
    type NodeCategory,
  } from '$lib/nodes/catalog';
  import { PORT_COLORS, PORT_LABELS, type PortDataType } from '$lib/types/blueprint';
  // Shared module-level state: every `Blueprint Input` node in the
  // graph reads this array to decide which output handles to draw.
  // Source of truth is `bpMeta.metadata.inputs`; the page mirrors
  // updates from the Inspector into this state via `$effect`.
  import {
    blueprintInputs,
    wiredPortKeys,
  } from '$lib/utils/blueprint-context.svelte';
  // `open` is the JS-side of `tauri-plugin-dialog`. We import it as
  // `openDialog` to avoid colliding with the `<dialog>` element / any
  // future DOM-native dialog helper we may add.
  import { open as openDialog } from '@tauri-apps/plugin-dialog';

  type Props = NodeProps;

  let { id, data, selected }: Props = $props();

  // Svelte Flow v0.1.x context — invoked at top-level inside a node
  // component (already lives inside <SvelteFlow>).
  const { updateNodeData, deleteElements } = useSvelteFlow();

  // Remove this node and any edges that touch it from the writable
  // store the host owns. Svelte Flow also clears the selection.
  function deleteThis(event: MouseEvent) {
    event.stopPropagation();
    deleteElements({ nodes: [{ id }] });
  }

  // Svelte Flow forwards plain objects as `data`; cast once and reuse.
  const d = $derived((data ?? {}) as NodeData);

  const def = $derived(getDef(d.kind));
  const headerColor = $derived(
    CATEGORY_COLORS[(def?.category ?? ('IO' as NodeCategory)) as NodeCategory],
  );

  function updateParam(key: string, value: unknown) {
    updateNodeData(id, () => ({
      params: { ...(d.params ?? {}), [key]: value },
    }));
  }

  // For `blueprint.input` nodes: the source-port set is owned by the
  // PARENT blueprint's `metadata.inputs`, summarised here through
  // `portFromBlueprintInput` so `out.type` lands safely inside the
  // `PortDataType` union. Caches here so the each block is iteration-
  // cheap on graphs with many Blueprint Input nodes.
  const blueprintInputPorts = $derived(
    blueprintInputs.value.map(portFromBlueprintInput),
  );

  // Repeatable-input state. `null` cfg means a fixed-shape node; the
  // rest of the render branch falls through to the legacy `each` loop.
  const repeatCfg = $derived(def?.repeatableInputs ?? null);
  const repeatCount = $derived(
    repeatCfg ? (resolvedRepeatCount(def, d.params ?? {}) ?? repeatCfg.min) : 0,
  );

  /** Read the per-port inline-default value from `params[<port_id>]`.
   *  Returns the empty string when the param entry is missing OR
   *  when its rendered type doesn't match the port type (so a
   *  stale string in `params.a` on a number port reads back as
   *  the empty string — the runner-level fallback would then
   *  parse it via `Number(...)` itself). */
  function readInlineValue(
    params: Record<string, unknown> | undefined,
    portId: string,
    type: PortDataType,
  ): string {
    const raw = params?.[portId];
    if (raw === undefined || raw === null) return '';
    if (type === 'number') {
      const n = typeof raw === 'number' ? raw : Number(raw);
      return Number.isFinite(n) ? String(n) : '';
    }
    if (type === 'string') {
      return String(raw);
    }
    return '';
  }

  /** Persist the per-port inline-default value into `params[<port_id>]`.
   *  Empty string → undefined (removed-on-save semantics so the
   *  runner's `params.get(<port_id>).unwrap_or(&Null)` path
   *  resolves to Null when the user clears the field, matching
   *  the previous "unwired → Null" default). NaN after a number
   *  parse is treated the same way (cleared). */
  function writeInlineValue(
    portId: string,
    type: PortDataType,
    raw: string,
  ): void {
    let parsed: string | number | undefined;
    if (raw === '') {
      parsed = undefined;
    } else if (type === 'number') {
      const n = Number(raw);
      parsed = Number.isFinite(n) ? n : undefined;
    } else if (type === 'string') {
      parsed = raw;
    } else {
      parsed = undefined;
    }
    updateNodeData(id, () => ({
      params: { ...(d.params ?? {}), [portId]: parsed },
    }));
  }

  /** Set of port ids currently wired into THIS node. Read from
   *  the module-level projection that `Editor.svelte`'s `$effect`
   *  keeps in sync with the edges store. Per-node read so a port
   *  is only re-evaluated when the slice for this node changes
   *  (cheap on big graphs). */
  const wiredPortsForThisNode = $derived(
    wiredPortKeys.value.get(id) ?? new Set<string>(),
  );

  /** Bump the count param up or down, clamped to the configured range. */
  function bumpRepeatCount(delta: number) {
    if (!repeatCfg) return;
    const next = Math.max(
      repeatCfg.min,
      Math.min(repeatCfg.max, repeatCount + delta),
    );
    const key = repeatCfg.countKey ?? 'count';
    updateParam(key, next);
  }

  /**
   * Open the OS-native dialog and capture the user's selection into
   * `params.path`. The mode controls what it opens:
   *   * `file`            → single-file dialog → `string`
   *   * `folder`          → folder-picker    → `string`
   *   * `multiple-files`  → multi-select     → `string[]`
   *
   * On cancel (`null` from `open`), we keep the prior state — the
   * button is non-destructive so accidentally escaping the dialog
   * does not blank the user's previous selection.
   *
   * NOTE: We branch the options object per-mode rather than passing
   * `{ directory: true, multiple: false }` together. The documented
   * `open()` pattern for a folder picker is `{ directory: true }` alone
   * (no `multiple`) per the `OpenDialogOptions` type signature — any
   * extra flags are outside the documented folder-pick contract and
   * may interact badly with the host platform's native dialog. The
   * symptom observed by the user was that in folder mode, the dialog
   * accepted no folder selection at all. Per-mode branching sidesteps
   * the cross-mode contamination entirely — folder mode now sends
   * exactly the documented shape.
   *
   * DIAGNOSTICS: every variant of the outcome is logged with
   * `console.{debug,warn,error}` so a runtime regression is
   * debuggable from DevTools. The previous version silently dropped
   * empty strings / non-string returns, which made "folder mode
   * accepts nothing" indistinguishable from "user cancelled". With
   * diagnostics, the user can read the console and tell us whether
   * the dialog returned `null` (cancel), `""` (empty), the actual
   * path, or an unexpected type — each maps to a known cause.
   */
  async function browsePaths(): Promise<void> {
    const mode =
      (d.params.mode as 'file' | 'folder' | 'multiple-files' | undefined) ?? 'file';
    const titleMap = {
      file: 'Pick a file',
      folder: 'Pick a folder',
      'multiple-files': 'Pick one or more files',
    } as const;
    // Per-mode options object — see the JSDoc above `browsePaths`
    // for the rationale behind mutually-exclusive `directory` /
    // `multiple` flags.
    const options =
      mode === 'folder'
        ? { directory: true, title: titleMap[mode] }
        : mode === 'multiple-files'
          ? { multiple: true, title: titleMap[mode] }
          : { title: titleMap[mode] };
    console.debug('[file.pathPicker] click', { mode, options });
    let result: unknown;
    try {
      result = await openDialog(options);
    } catch (err) {
      // Plugin errors are rare (e.g. plugin not registered, denied
      // permission). Surface them loudly so the user can see WHY the
      // dialog didn't return anything (vs. silently failing).
      console.error('[file.pathPicker] openDialog threw:', err);
      return;
    }
    console.debug('[file.pathPicker] returned', result, 'type:', typeof result);
    // Handle every concrete shape `open()` documents returning. The
    // body is a flat if/else if chain (not a switch(true) on a
    // boolean predicate) because predicate-dispatch via switch(true)
    // reads as a hack and most TS style guides prefer if/else if for
    // shape discrimination.
    if (result === null || result === undefined) {
      // User cancelled. Keep prior state intact.
      return;
    }
    if (typeof result === 'string') {
      // An empty string here is normally cancel-on-some-OSes
      // behaviour (rare) but worth a warning so a future plugin
      // regression is debuggable from DevTools.
      if (result.length === 0) {
        console.warn('[file.pathPicker] open returned an empty string', { mode });
        return;
      }
      updateParam('path', result);
      return;
    }
    if (Array.isArray(result)) {
      // `[]` cancel-result is a legitimate no-op — the user can use
      // the Clear (×) button for EXPLICIT blanking instead.
      if (result.length === 0) return;
      // Enforce "every slot is a string" at the boundary instead of
      // casting `unknown[]` to `string[]` blindly. A buggy plugin
      // version could surface a heterogeneous array (e.g. `[null,
      // "/path"]`); rather than feeding that lie to downstream code,
      // we drop non-string slots here AND log a per-slot warning so
      // the user can see what shape their picker is returning. The
      // Rust executor also enforces this invariant but doing it at
      // the UI boundary keeps the failure mode local to the picker.
      const strings: string[] = [];
      for (let i = 0; i < result.length; i++) {
        const v = result[i];
        if (typeof v === 'string') {
          strings.push(v);
        } else {
          console.warn(
            '[file.pathPicker] dropped non-string slot from open() result',
            { mode, index: i, value: v },
          );
        }
      }
      if (strings.length > 0) updateParam('path', strings);
      return;
    }
    console.error(
      '[file.pathPicker] open returned an unexpected shape',
      { mode, value: result },
    );
  }
</script>

<div class="node" class:selected class:kind-io={d.kind?.startsWith('io.')}>
  <div class="header" style="--header-color: {headerColor}">
    <span class="kind">{d.label || def?.label || d.kind}</span>
    <button
      class="del"
      type="button"
      title="Delete this node (Esc to cancel)"
      aria-label="Delete node"
      onclick={deleteThis}
    >×</button>
  </div>

  <div class="body">
    {#if repeatCfg}
      {#each Array(repeatCount) as _, i (i)}
        <div class="row">
          <Handle
            type="target"
            position={Position.Left}
            id={`${repeatCfg.prefix}_${i + 1}`}
            class="port port-{repeatCfg.type}"
            style="--port-color: {PORT_COLORS[repeatCfg.type]}"
          />
          <span class="port-label" title={PORT_LABELS[repeatCfg.type]}>
            {repeatCfg.label} {i + 1}
          </span>
        </div>
      {/each}
      <div class="row controls">
        <span class="badge">
          {repeatCfg.label.toLowerCase()}s: <strong>{repeatCount}</strong>
        </span>
        <button
          type="button"
          class="ctrl-btn"
          onclick={() => bumpRepeatCount(-1)}
          disabled={repeatCount <= repeatCfg.min}
          title="Remove one field"
          aria-label="Remove one field">−</button>
        <button
          type="button"
          class="ctrl-btn"
          onclick={() => bumpRepeatCount(+1)}
          disabled={repeatCount >= repeatCfg.max}
          title="Add one field (max {repeatCfg.max})"
          aria-label="Add one field">+</button>
      </div>
    {:else}
      {#each (d.inputs ?? []) as input (input.id)}
        <!-- Type-exclusion gate lives at the line below
             (`input.type === 'string' || input.type === 'number'`).
             Rationale (catalog-driven): boolean/array/binary/json
             are not currently exposed as INPUT ports on any node
             (their literals cover the source side on the canvas),
             `any` is shape-ambiguous, `string-or-array` is
             explicitly dual-shape (a typed scalar default would
             not match the wire shape), and `exec` carries no
             data at all. If a future category exposes a
             boolean/array/etc. INPUT port, re-evaluate this gate
             and add the per-type widget — these helpers
             (readInlineValue/writeInlineValue) accept the full
             PortDataType union and narrow safely. -->
        <div class="row">
          <Handle
            type="target"
            position={Position.Left}
            id={input.id}
            class="port port-{input.type}"
            style="--port-color: {PORT_COLORS[input.type]}"
          />
          <span class="port-label" title={PORT_LABELS[input.type]}>{input.name}</span>
          {#if input.type === 'string' || input.type === 'number'}
            <!-- Blender-style "input deflection" — every scalar
                 input gains an inline editable default. Disables
                 when the port is wired (the wire wins, see the
                 runner.rs fallback). Value persists as
                 params[<port_id>] which the runner threads
                 through as a default when the port is unwired.
                 Coerces non-numeric input on number ports to ""
                 (NaN becomes empty rather than throwing). -->
            <input
              class="inline-value"
              type={input.type === 'number' ? 'number' : 'text'}
              disabled={wiredPortsForThisNode.has(input.id)}
              placeholder={input.type === 'number' ? '0' : '...'}
              value={readInlineValue(d.params, input.id, input.type)}
              onmousedown={(e) => e.stopPropagation()}
              onpointerdown={(e) => e.stopPropagation()}
              onclick={(e) => e.stopPropagation()}
              oninput={(e) =>
                writeInlineValue(
                  input.id,
                  input.type,
                  e.currentTarget.value,
                )}
            />
          {/if}
        </div>
      {/each}
    {/if}

    {#if d.kind === 'literal.number'}
      <div class="row editor">
        <input
          type="number"
          step="0.1"
          value={(d.params.value as number | undefined) ?? 0}
          oninput={(e) => updateParam('value', Number(e.currentTarget.value))}
        />
      </div>
    {:else if d.kind === 'literal.string'}
      <div class="row editor">
        <input
          type="text"
          placeholder="…"
          value={(d.params.value as string | undefined) ?? ''}
          oninput={(e) => updateParam('value', e.currentTarget.value)}
        />
      </div>
    {:else if d.kind === 'literal.boolean'}
      <div class="row editor">
        <label class="inline-check">
          <input
            type="checkbox"
            checked={!!d.params.value}
            onchange={(e) => updateParam('value', e.currentTarget.checked)}
          />
          <span>{d.params.value ? 'true' : 'false'}</span>
        </label>
      </div>
    {:else if d.kind === 'literal.json'}
      <div class="row editor">
        <input
          type="text"
          spellcheck="false"
          value={(d.params.value as string | undefined) ?? 'null'}
          oninput={(e) => updateParam('value', e.currentTarget.value)}
        />
      </div>
    {:else if d.kind === 'math.compare'}
      <div class="row editor">
        <select
          value={(d.params.op as string | undefined) ?? 'eq'}
          onchange={(e) => updateParam('op', e.currentTarget.value)}
        >
          <option value="eq">=</option>
          <option value="neq">≠</option>
          <option value="gt">&gt;</option>
          <option value="gte">≥</option>
          <option value="lt">&lt;</option>
          <option value="lte">≤</option>
        </select>
      </div>
    {:else if d.kind === 'string.split'}
      <div class="row editor">
        <span class="badge">sep:</span>
        <input
          type="text"
          value={(d.params.sep as string | undefined) ?? ','}
          oninput={(e) => updateParam('sep', e.currentTarget.value)}
        />
      </div>
    {:else if d.kind === 'string.join'}
      <div class="row editor">
        <span class="badge">sep:</span>
        <input
          type="text"
          value={(d.params.sep as string | undefined) ?? ' '}
          oninput={(e) => updateParam('sep', e.currentTarget.value)}
        />
      </div>
    {:else if d.kind === 'string.replace'}
      <div class="row editor replace-row">
        <input
          type="text"
          placeholder="find"
          value={(d.params.find as string | undefined) ?? ''}
          oninput={(e) => updateParam('find', e.currentTarget.value)}
        />
        <span class="arrow" aria-hidden="true">→</span>
        <input
          type="text"
          placeholder="replace"
          value={(d.params.replace as string | undefined) ?? ''}
          oninput={(e) => updateParam('replace', e.currentTarget.value)}
        />
      </div>
    {:else if d.kind === 'array.filter'}
      <div class="row editor">
        <span class="badge">≥</span>
        <input
          type="number"
          step="0.1"
          value={(d.params.threshold as number | undefined) ?? 0}
          oninput={(e) => updateParam('threshold', Number(e.currentTarget.value))}
        />
      </div>
    {:else if d.kind === 'io.logger'}
      <div class="row editor">
        <span class="badge">{(d.params.label as string | undefined) ?? 'log'}:</span>
      </div>
    {:else if d.kind === 'image.format'}
      <div class="row editor">
        <span class="badge">fmt:</span>
        <select
          value={(d.params.format as string | undefined) ?? 'png'}
          onchange={(e) => updateParam('format', e.currentTarget.value)}
        >
          <option value="png">png</option>
          <option value="jpeg">jpeg</option>
          <option value="jpg">jpg</option>
          <option value="webp">webp</option>
          <option value="gif">gif</option>
          <option value="bmp">bmp</option>
        </select>
        <span class="badge">q:</span>
        <input
          type="number"
          min="1"
          max="100"
          step="1"
          value={(d.params.quality as number | undefined) ?? 90}
          oninput={(e) => updateParam('quality', Number(e.currentTarget.value))}
        />
      </div>
    {:else if d.kind === 'file.pathPicker'}
      <div class="row editor path-picker-row">
        <span class="badge">mode:</span>
        <select
          class="mode-select"
          value={(d.params.mode as string | undefined) ?? 'file'}
          onchange={(e) => {
            const next = (e.currentTarget as HTMLSelectElement).value;
            // Mode change invalidates the prior selection: switching
            // from `multiple-files` (array on disk) to `file` (string
            // expected) would otherwise leave a stale array rendered
            // next to a single-file label. Drop `path` so the
            // chip-list shows "(no path selected yet)" until the
            // user clicks Browse again. The two writes are folded
            // into a SINGLE updateNodeData call so the second
            // spread can't race against the first (each call to
            // `updateParam` reads `d.params` fresh, so two
            // successive calls can clobber each other when the
            // reactive `d` hasn't re-derived yet).
            updateNodeData(id, () => ({
              params: { ...(d.params ?? {}), mode: next, path: undefined },
            }));
          }}
        >
          <option value="file">file</option>
          <option value="folder">folder</option>
          <option value="multiple-files">multiple-files</option>
        </select>
        <button
          type="button"
          class="browse-btn"
          title={`Open system dialog to pick a ${
            (d.params.mode as string | undefined) ?? 'file'
          }`}
          onclick={browsePaths}
        >Browse…</button>
      </div>
      {#if d.params.path !== undefined}
        {@const pp = d.params.path}
        <div class="row path-list">
          {#if Array.isArray(pp)}
            {#if pp.length === 0}
              <span class="path-empty">(cleared)</span>
            {:else}
              {#each pp as p, i (i)}
                <span class="path-chip" title={String(p)}>{String(p)}</span>
              {/each}
            {/if}
          {:else if typeof pp === 'string' && pp.length > 0}
            <span class="path-chip" title={pp}>{pp}</span>
          {:else}
            <span class="path-empty">(no path selected yet)</span>
          {/if}
          <button
            type="button"
            class="clear-btn"
            title="Clear selected paths"
            aria-label="Clear selected paths"
            disabled={pp === undefined ||
              (Array.isArray(pp) && pp.length === 0) ||
              (typeof pp === 'string' && pp.length === 0)}
            onclick={() => updateParam('path', [])}
          >×</button>
        </div>
      {/if}
    {:else if d.kind === 'file.delete'}
      <div class="row editor">
        <textarea
          class="paths-textarea"
          spellcheck="false"
          placeholder={'One path per line.\nConnect a `paths` port to override this list.'}
          value={(d.params.paths as string | undefined) ?? ''}
          oninput={(e) => updateParam('paths', (e.currentTarget as HTMLTextAreaElement).value)}
        ></textarea>
      </div>
      <div class="row editor">
        <label
          class="inline-check"
          title={`With recursive=${d.params.recursive ? 'true' : 'false'} — ${
            d.params.recursive
              ? 'removes directories AND their contents (fs::remove_dir_all); use with care.'
              : 'safe mode (fs::remove_file); errors on directories to prevent accidents.'
          }`}
        >
          <input
            type="checkbox"
            checked={!!d.params.recursive}
            onchange={(e) => updateParam('recursive', e.currentTarget.checked)}
          />
          <span>recursive</span>
        </label>
      </div>
    {/if}

    {#each (d.kind === 'blueprint.input' ? blueprintInputPorts : (d.outputs ?? [])) as out (out.id)}
      <div class="row right">
        <span class="port-label">{out.name}</span>
        <Handle
          type="source"
          position={Position.Right}
          id={out.id}
          class="port port-{out.type}"
          style="--port-color: {PORT_COLORS[out.type]}"
        />
      </div>
    {/each}
  </div>
</div>

<style>
  .node {
    --bg: #ffffff;
    --border: #c8c8c8;
    --border-selected: #2d7cf1;
    width: 180px;
    background: var(--bg);
    border: 1.5px solid var(--border);
    border-radius: 7px;
    font-size: 12px;
    color: #222;
    box-shadow:
      0 1px 0 rgba(0, 0, 0, 0.05),
      0 2px 4px rgba(0, 0, 0, 0.04);
    font-family: 'Inter', system-ui, sans-serif;
    /* Do NOT set `overflow: hidden`: Svelte Flow's handles are positioned
       absolutely outside the node bounding box, and clipping them makes
       the ports visually cut off at the node edge. */
    overflow: visible;
    transition:
      border-color 0.12s ease,
      box-shadow 0.12s ease;
  }
  .node.selected {
    /* Selection indicator = border colour only. No halo, no drop shadow.
       The previous box-shadow stack painted a soft blue 2px ring around
       the node that the user kept seeing as 'el contorno que no me
       gusta'; this drop also removes the unresolved Svelte Flow
       --xy-node-boxshadow-selected-default. Net visual on selected =
       border colour changes to --border-selected (#2d7cf1), nothing
       else changes. */
    border-color: var(--border-selected);
  }

  .header {
    position: relative;
    background: var(--header-color, #888);
    color: white;
    padding: 5px 10px;
    font-weight: 600;
    text-align: center;
    font-size: 11.5px;
    letter-spacing: 0.02em;
    text-shadow: 0 1px 0 rgba(0, 0, 0, 0.12);
  }

  /* Compact delete button anchored top-right of the header. Subtle
     by default so the node reads clean; turns red on hover. Visible
     even on unselected nodes so the user does not have to pick first. */
  .del {
    position: absolute;
    top: 50%;
    right: 6px;
    transform: translateY(-50%);
    width: 18px;
    height: 18px;
    border-radius: 50%;
    border: none;
    background: rgba(255, 255, 255, 0.18);
    color: white;
    font-size: 14px;
    line-height: 14px;
    font-weight: 700;
    font-family: inherit;
    padding: 0;
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    opacity: 0.55;
    transition:
      opacity 0.1s ease,
      background 0.1s ease,
      transform 0.05s ease;
  }
  .node:hover .del,
  .node.selected .del {
    opacity: 0.95;
  }
  .del:hover {
    opacity: 1 !important;
    background: rgba(220, 38, 38, 0.92);
  }
  .del:active {
    transform: translateY(-50%) scale(0.92);
  }
  .del:focus-visible {
    outline: 2px solid #2d7cf1;
    outline-offset: 1px;
  }
  .kind {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .body {
    padding: 6px 0;
  }

  .row {
    display: flex;
    align-items: center;
    justify-content: flex-start;
    gap: 8px;
    padding: 4px 14px;
    min-height: 22px;
    position: relative;
  }
  .row.right {
    justify-content: flex-end;
  }

  .row.editor {
    padding: 6px 14px;
    border-top: 1px dashed #e5e7eb;
    border-bottom: 1px dashed #e5e7eb;
    background: #fafafa;
  }

  .row.controls {
    justify-content: flex-end;
    gap: 4px;
    padding: 4px 14px;
    border-top: 1px dashed #e5e7eb;
    background: #fafafa;
  }
  .row.controls .badge {
    margin-right: auto;
  }

  .port-label {
    font-size: 11px;
    color: #374151;
  }

  /* Inline editable default for scalar (string / number) input
     ports. Sits on the same row as the label, flex-grows to fill
     the remaining horizontal space, and visually mutes when the
     port is wired (the wire wins). The runner-side counterpart
     in `runner.rs` falls back to `params[<port_id>]` exactly when
     this field is enabled and non-empty. */
  .inline-value {
    flex: 1 1 0;
    min-width: 0;
    height: 18px;
    border: 1px solid #d1d5db;
    border-radius: 3px;
    padding: 0 5px;
    font-size: 11px;
    font-family: 'JetBrains Mono', monospace;
    background: white;
    color: #111;
    outline: none;
    transition:
      border-color 0.1s ease,
      box-shadow 0.1s ease,
      background 0.1s ease,
      color 0.1s ease;
  }
  .inline-value:focus:not(:disabled) {
    border-color: #2d7cf1;
    box-shadow: 0 0 0 2px rgba(45, 124, 241, 0.2);
  }
  .inline-value:disabled {
    background: #f3f4f6;
    color: #9ca3af;
    cursor: not-allowed;
    border-color: #e5e7eb;
  }
  .inline-value::placeholder {
    color: #c4c4c4;
    font-style: italic;
  }
  /* Tighten the row's gap so the new field fits alongside an
     already-present label without spilling out of the node card. */
  .row:has(.inline-value) {
    gap: 6px;
  }

  .badge {
    font-family: 'JetBrains Mono', monospace;
    background: #f3f4f6;
    border: 1px solid #d1d5db;
    border-radius: 3px;
    padding: 1px 5px;
    font-size: 11px;
    color: #374151;
  }

  input[type='text'],
  input[type='number'],
  select {
    width: 100%;
    border: 1px solid #d1d5db;
    border-radius: 3px;
    padding: 2px 5px;
    font-size: 11px;
    background: white;
    color: #111;
    font-family: inherit;
  }
  input:focus,
  select:focus {
    outline: none;
    border-color: #2d7cf1;
    box-shadow: 0 0 0 2px rgba(45, 124, 241, 0.2);
  }

  /* Blender-style port shapes — type tells shape, color already encoded. */
  :global(.svelte-flow__handle.port) {
    width: 13px;
    height: 13px;
    background: var(--port-color, #888);
    border: 1.5px solid white;
    box-shadow: 0 1px 2px rgba(0, 0, 0, 0.18);
  }
  :global(.svelte-flow__handle.port-number) {
    border-radius: 50%;
  }
  :global(.svelte-flow__handle.port-string) {
    border-radius: 2px;
  }
  :global(.svelte-flow__handle.port-boolean) {
    border-radius: 0;
    transform: rotate(45deg);
  }
  :global(.svelte-flow__handle.port-array) {
    clip-path: polygon(50% 0, 100% 100%, 0 100%);
    border-radius: 0;
  }
  :global(.svelte-flow__handle.port-any) {
    background: transparent;
    border: 2px solid var(--port-color, #6b7280);
    border-radius: 50%;
  }
  /* Binary: solid violet hexagon so it's instantly distinguishable
     from translucent `any` (gray) and sharp-edged `string` (green).
     clip-path unsharply cuts the corners; the white inner border
     remains visible because of the original `border: 1.5px solid
     white` rule above. */
  :global(.svelte-flow__handle.port-binary) {
    border-radius: 0;
    clip-path: polygon(
      25% 0%, 75% 0%,
      100% 50%, 75% 100%,
      25% 100%, 0% 50%
    );
  }
  /* `string-or-array`: pentagon (square base + apex top) that fuses
     the visual languages of `string` (square) and `array`
     (triangle) so the dual-shape contract is intuitive without
     needing a tooltip. Distinct at 13x13 from circle/number,
     square/string, diamond/boolean, triangle/array,
     hexagon/binary. Five polygon points: top apex → right-shoulder
     → bottom-right → bottom-left → left-shoulder → apex. */
  :global(.svelte-flow__handle.port-string-or-array) {
    border-radius: 0;
    clip-path: polygon(50% 0, 100% 50%, 100% 100%, 0 100%, 0 50%);
  }
  /* `exec` — solid black circle. Distinguished from `number`
     (which is also a circle) by TWO intentional cues so its
     ordering-only meaning reads first-glance:
       1. Colour — data ports use the palette colours per
          PORT_COLORS, exec is uniformly #000.
       2. No white inner border — the base `.port` rule ships
          with a 1.5px white border; exec overrides to
          `border: 0` so the solid black fills the entire
          13x13 shape. Number keeps the white border for
          canvas legibility.
     No clip-path (we want a true circle, not the chevron
     that previously lived here). Side (left target / right
     source) still encodes input vs output, mirroring every
     other port's "shape is symmetric, side is the handle
     position" convention. */
  :global(.svelte-flow__handle.port-exec) {
    background: #000000;
    border: 0;
    border-radius: 50%;
    clip-path: none;
  }
  /* Mirror `.port-exec` colour values in case the Inspector ever reads
     the handle directly. The source of truth for the rendered shape
     is this CSS; `PORT_COLORS.exec` (in src/lib/types/blueprint.ts)
     pins the same `#000000` so the Inspector swatch matches. */

  .inline-check {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 11px;
  }

  /* Compact +/− buttons for the repeatable-input controls row. Same scale
     as the delete `×` button on the header but on a light background, so
     we use a white card with the same focus outline as form inputs. */
  .ctrl-btn {
    width: 20px;
    height: 20px;
    font-size: 13px;
    font-weight: 700;
    font-family: 'JetBrains Mono', monospace;
    background: white;
    border: 1px solid #d1d5db;
    border-radius: 3px;
    cursor: pointer;
    padding: 0;
    line-height: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    color: #374151;
    transition:
      background 0.1s ease,
      border-color 0.1s ease,
      color 0.1s ease,
      transform 0.05s ease;
  }
  .ctrl-btn:hover:not(:disabled) {
    background: #eef2ff;
    border-color: #2d7cf1;
    color: #2d7cf1;
  }
  .ctrl-btn:active:not(:disabled) {
    transform: scale(0.92);
  }
  .ctrl-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }
  .ctrl-btn:focus-visible {
    outline: 2px solid #2d7cf1;
    outline-offset: 1px;
  }

  /* `string.replace` inline editor shows two stacked text inputs
     linked by an arrow chip. The two inputs share the row's width
     via flex:1; the arrow is fixed-width and centred. Tight row
     helps the visual flow feel like "before → after". */
  .replace-row {
    gap: 4px;
  }
  .replace-row input[type='text'] {
    flex: 1 1 0;
    min-width: 0;
  }
  .arrow {
    font-family: 'JetBrains Mono', monospace;
    color: #6b7280;
    font-size: 12px;
    flex: 0 0 auto;
    user-select: none;
  }

  /* `file.pathPicker` inline editor: a wide primary button on the
     row alongside a compact mode-select, then a chip-list row
     below showing the user's selected paths. The chip-list row
     is full-width so long paths don't crowd the mode selector. */
  .path-picker-row {
    gap: 4px;
  }
  .path-picker-row .mode-select {
    flex: 0 0 auto;
    width: auto;
    min-width: 0;
  }
  .browse-btn {
    flex: 1 1 0;
    min-width: 0;
    font-family: inherit;
    font-size: 11px;
    font-weight: 600;
    background: white;
    border: 1px solid #2d7cf1;
    border-radius: 3px;
    padding: 2px 8px;
    color: #2d7cf1;
    cursor: pointer;
    transition:
      background 0.1s ease,
      color 0.1s ease,
      transform 0.05s ease;
  }
  .browse-btn:hover {
    background: #2d7cf1;
    color: white;
  }
  .browse-btn:active {
    transform: scale(0.97);
  }
  .browse-btn:focus-visible {
    outline: 2px solid #2d7cf1;
    outline-offset: 1px;
  }
  .path-list {
    flex-wrap: wrap;
    gap: 4px;
    padding: 4px 14px;
    background: #f4f4f5;
    border-top: 1px dashed #e5e7eb;
    align-items: center;
  }
  .path-chip {
    font-family: 'JetBrains Mono', monospace;
    font-size: 10px;
    background: white;
    border: 1px solid #d1d5db;
    border-radius: 3px;
    padding: 1px 5px;
    color: #374151;
    max-width: 130px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    display: inline-block;
  }
  .path-empty {
    font-style: italic;
    font-size: 10px;
    color: #9ca3af;
  }
  .clear-btn {
    margin-left: auto;
    width: 18px;
    height: 18px;
    font-size: 12px;
    line-height: 1;
    background: white;
    border: 1px solid #d1d5db;
    border-radius: 50%;
    color: #6b7280;
    cursor: pointer;
    padding: 0;
    flex: 0 0 18px;
    display: flex;
    align-items: center;
    justify-content: center;
    transition:
      background 0.1s ease,
      border-color 0.1s ease,
      color 0.1s ease;
  }
  .clear-btn:hover:not(:disabled) {
    background: #fee2e2;
    border-color: #ef4444;
    color: #ef4444;
  }
  .clear-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  /* `file.delete` inline editor: a paths textarea (one path per
     line) for ad-hoc deletes + a recursive checkbox whose tooltip
     spells out the safety trade-off. The textarea preserves newlines
     verbatim so the executor's split-on-`\n` produces the right
     array length when the user types paths manually. resize: vertical
     keeps the default node footprint compact but lets the user grow
     the textarea on a big batch without forcing it on every node. */
  .paths-textarea {
    width: 100%;
    min-height: 52px;
    resize: vertical;
    border: 1px solid #d1d5db;
    border-radius: 3px;
    padding: 3px 6px;
    font-size: 11px;
    font-family: 'JetBrains Mono', monospace;
    background: white;
    color: #111;
    line-height: 1.4;
  }
  .paths-textarea:focus {
    outline: none;
    border-color: #2d7cf1;
    box-shadow: 0 0 0 2px rgba(45, 124, 241, 0.2);
  }
  .paths-textarea::placeholder {
    color: #9ca3af;
    white-space: pre;
    font-style: italic;
  }
</style>
