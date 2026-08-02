<!--
  Pre-Run modal: appears with one field per declared
  `BlueprintInput`. Renders different input shapes depending on the
  BlueprintInput.type:

    * `file`        → text input + Browse… button (opens native
                      single-file dialog).
    * `folder`      → text input + Browse… (native folder picker).
    * `multi-file`  → chip-list + + Add files button (native
                      multi-select; chips accumulate across clicks).
    * `number`      → numeric input.
    * `boolean`     → select (false/true).
    * `string`      → text input.
    * `array`/`any` → JSON textarea.

  The text-input rows for `file`/`folder` let the user paste a path
  manually too (some workflows prefer typing). The Browse buttons
  pre-fill the same field with the picked value.

  Anchored absolutely to whatever parent the consumer places it inside
  (typically `.canvas-wrap`). Position is `fixed inset:0` so it covers
  the canvas viewport regardless of layout. Esc cancels, backdrop
  click cancels.
-->
<script lang="ts">
  import type { BlueprintInput } from '$lib/types/blueprint';
  import { open as openDialog } from '@tauri-apps/plugin-dialog';
  import {
    loadInputHistory,
    saveInputHistory,
  } from '$lib/utils/inputs-history';

  /** A single row in the modal. `type` carries the full
   *  BlueprintInput['type'] union so the dialog branches per kind
   *  (including picker-style rows for `file` / `folder` / `multi-file`).
   *  No defensive guard needed — the union is exactly what the
   *  Inspector and Dashboard write. */
  export interface InputRequest {
    name: string;
    label: string;
    type: BlueprintInput['type'];
    nodeId: string;
  }

  interface Props {
    inputs: InputRequest[];
    onSubmit: (values: Record<string, unknown>) => void;
    onCancel: () => void;
    /**
     * Per-blueprint key used to round-trip the last submitted
     * values through localStorage. When supplied, the dialog
     * merges the persisted values on open (each input matching
     * the CURRENT `inputs` list is prefilled with its stored
     * value; inputs not in storage stay at their per-type
     * defaults) and saves the parsed value map on submit. When
     * omitted or null, the dialog is behaviourally identical to
     * before — no persistence, no merge, no save. Dashboard
     * passes the absolute file path; Editor passes
     * `currentFilePath || bpMeta.id` so the same blueprint always
     * re-opens with its last values.
     */
    storageKey?: string | null;
  }
  let { inputs, onSubmit, onCancel, storageKey = null }: Props = $props();

  /** Per-input raw buffer. For scalar picker-style inputs (`file`,
   *  `folder`, `string`, …) this is a string; for `multi-file` AND
   *  `array` it holds a `string[]` (multi-file accumulates chips
   *  across Browse clicks; array gives one row per slot the user
   *  typed a value into). Both branches need the same inner shape
   *  so the typed `OutsideIn` of the value is a simple `string[]`.
   *  `any` is left as a string buffer (the user can paste any
   *  JSON shape; the legacy JSON textarea UX survives for that
   *  case). */
  let values = $state<Record<string, string | string[]>>({});

  /** Coerce a value pulled out of `localStorage` (JSON-parsed
   *  by `loadInputHistory`) back into the dialog's raw-buffer
   *  shape (`string | string[]`). Returns `null` when the
   *  persisted shape doesn't match this row's expected shape,
   *  in which case the caller falls back to the per-type
   *  default (`''`, `[]`, `['']`). Tight check on `Array.isArray`
   *  protects against an older schema that serialised `array`
   *  values as a single string. */
  function coercePersistedIntoRaw(
    raw: unknown,
    req: InputRequest,
  ): string | string[] | null {
    if (req.type === 'array' || req.type === 'multi-file') {
      if (!Array.isArray(raw)) return null;
      return raw.filter((x): x is string => typeof x === 'string');
    }
    // Scalar shapes — undo the parseFor-coercion from the last
    // run so the input control renders the value verbatim.
    if (typeof raw === 'string') return raw;
    if (typeof raw === 'number') return String(raw);
    if (typeof raw === 'boolean') return String(raw);
    return null;
  }

  // Reset the form to fresh defaults whenever the inputs prop shifts
  // (the parent opens the dialog with a new set), then layer the
  // persisted last-run values on top so the dialog opens with the
  // user's previous submission already filled in.
  //   * `multi-file` → `[]`  (chip list shows "no files yet" hint)
  //   * `array`      → `['']` if the persisted merge leaves it empty,
  //                     otherwise whatever the user ran last time
  //   * everything else → `''`
  //
  // Iterating over the CURRENT `inputs` (not over `persisted`) means
  // schema drift is harmless: stale keys for inputs the user removed
  // or renamed since the last run are simply not consumed, and new
  // inputs (declared after the last run) keep their fresh default.
  $effect(() => {
    const fresh: Record<string, string | string[]> = {};
    for (const r of inputs) {
      if (r.type === 'multi-file') fresh[r.name] = [];
      else if (r.type === 'array') fresh[r.name] = [''];
      else fresh[r.name] = '';
    }
    if (storageKey) {
      const persisted = loadInputHistory(storageKey);
      if (persisted) {
        for (const r of inputs) {
          const raw = persisted[r.name];
          if (raw === undefined) continue;
          const coerced = coercePersistedIntoRaw(raw, r);
          if (coerced !== null) fresh[r.name] = coerced;
        }
        // If the previous run saved an empty `array` (because the
        // user submitted with no slots), the dialog would render
        // without any visible row — disorienting. Lift the empty
        // case back to one empty slot so the next-run UX is
        // identical to a pristine first run.
        for (const r of inputs) {
          if (
            r.type === 'array' &&
            Array.isArray(fresh[r.name]) &&
            (fresh[r.name] as string[]).length === 0
          ) {
            fresh[r.name] = [''];
          }
        }
      }
    }
    values = fresh;
  });

  /** Per-input validation error message (or `''` = no error). Used
   *  to surface inline red feedback under `array`/`any` textareas and
   *  `number` rows when the user types a value the Rust runner can't
   *  consume.
   *
   *  `$derived.by` (preferred over a manual `$state` + `$effect`)
   *  gives us proven reactive ordering: validation re-runs whenever
   *  `inputs` or `values` change, in Svelte's topological order,
   *  so a reset-and-revalidate on a fresh modal open can't show a
   *  flicker of the previous run's stale errors. */
  const parseErrors = $derived.by(() => {
    const next: Record<string, string> = {};
    for (const r of inputs) {
      const raw = values[r.name];
      if (r.type === 'any') {
        // `array` is no longer JSON-validated; each row is a
        // plain text field and empty slots are skipped at submit
        // time. The textarea-only validation survives for `any`
        // because that branch still consumes raw JSON.
        const trimmed = typeof raw === 'string' ? raw.trim() : '';
        if (trimmed === '') continue;
        try {
          JSON.parse(trimmed);
        } catch (e) {
          // Cap at 60 chars so a long JSON.parse error doesn't push
          // the modal's body out of layout; append a `…` when the
          // message actually got cut so the reader can tell why the
          // cause isn't fully explained (without the suffix, the
          // truncation reads like the full error).
          const errMsg = String(e);
          const shown = errMsg.length > 60 ? errMsg.slice(0, 60) + '…' : errMsg;
          next[r.name] = `Invalid JSON: ${shown}`;
        }
      } else if (r.type === 'number') {
        if (typeof raw !== 'string' || raw.trim() === '') continue;
        if (Number.isNaN(Number(raw))) {
          next[r.name] = 'Not a number';
        }
      }
    }
    return next;
  });

  /**
   * Open a `tauri-plugin-dialog` appropriate for the input's picker
   * kind and write the result to `values[name]`. Multi-file is
   * accumulated (existing array + new array) so the user can grow
   * the chip list across clicks without losing prior picks; other
   * kinds replace the value. Matches the documented shape of
   * `open()` from `@tauri-apps/plugin-dialog`:
   *   * `file`    → returns `string | null`
   *   * `folder`  → returns `string | null`
   *   * `multi`   → returns `string[] | null`
   */
  async function browsePath(req: InputRequest) {
    const isFolder = req.type === 'folder';
    const isMulti = req.type === 'multi-file';
    const options = isMulti
      ? { multiple: true, title: 'Pick files' }
      : isFolder
        ? { directory: true, title: 'Pick folder' }
        : { title: 'Pick file' };
    let result: unknown;
    try {
      result = await openDialog(options);
    } catch (err) {
      // Plugin errors are rare (plugin not registered, denied
      // permission). Surface them loudly so the user can see WHY
      // the dialog didn't return anything.
      console.error('[InputsDialog] openDialog threw:', err);
      return;
    }
    if (isMulti && Array.isArray(result)) {
      const existing = (values[req.name] as string[] | undefined) ?? [];
      const newPaths = result.filter((v): v is string => typeof v === 'string');
      values = {
        ...values,
        [req.name]: [...existing, ...newPaths],
      };
      return;
    }
    if (!isMulti && typeof result === 'string' && result.length > 0) {
      values = { ...values, [req.name]: result };
      return;
    }
    // Cancellation (null) or any other shape is a no-op — the
    // user keeps their prior state intact.
  }

  /** Generic index-based removal that works for ANY
   *  `string[]`-keyed buffer `values[name]` — the multi-file
   *  chip list AND the new `array` slot list BOTH funnel
   *  through this. Keeping the name neutral avoids the
   *  "Multi" prefix (which used to imply multi-file only)
   *  mislabels a future contributor reading the function in
   *  the context of an array slot list. Defensive on
   *  `undefined` to survive stale state during HMR or weird
   *  prop races. */
  function removeAt(name: string, idx: number) {
    const list = (values[name] as string[] | undefined) ?? [];
    values = {
      ...values,
      [name]: list.filter((_, i) => i !== idx),
    };
  }

  /** Edit a single slot of an `array`-type input. `idx` is the
   *  slot position so the helper is total over out-of-bounds
   *  callers (a future refactor that exposes a drag-reorder
   *  control would need to keep the same envelope). No-trim so
   *  mid-edit whitespace is preserved verbatim; trailing slots
   *  empty after trimming are SKIPPED at submit time (see
   *  `parseFor`), so leading/trailing whitespace never reaches
   *  the runner. */
  function updateArraySlot(name: string, idx: number, value: string) {
    const list = (values[name] as string[] | undefined) ?? [];
    const next = list.slice();
    if (idx < 0 || idx >= next.length) return;
    next[idx] = value;
    values = { ...values, [name]: next };
  }

  /** Append a fresh empty slot to an `array`-type input. Kept
   *  here rather than inlined so a future key-binding (e.g. Enter
   *  in the last slot appends a new one) can call the same
   *  function. */
  function addArraySlot(name: string) {
    const list = (values[name] as string[] | undefined) ?? [];
    values = { ...values, [name]: [...list, ''] };
  }

  /** Convert a per-input raw buffer to the typed-value the Rust
   *  runner expects. Multi-file passes the accumulated array
   *  through; picker / string are pass-throughs; numeric / boolean
   *  are coerced; array / any are JSON-parsed with a string
   *  fallback so the user sees what they typed. */
  function parseFor(req: InputRequest, raw: string | string[]): unknown {
    switch (req.type) {
      case 'multi-file':
        return Array.isArray(raw) ? raw : [];
      case 'file':
      case 'folder':
      case 'string':
        return typeof raw === 'string' ? raw : '';
      case 'number':
        if (typeof raw !== 'string' || raw.trim() === '') return null;
        return Number(raw);
      case 'boolean':
        return raw === 'true';
      case 'array': {
        // Slot-by-slot raw buffer. Trim each entry, drop empties
        // so an unfilled trailing slot doesn't leak through. The
        // runner already knows how to coerce strings into the
        // downstream type (numbers, paths, etc.), so we don't
        // Number()/Boolean() here — that decision belongs to the
        // type that consumes the array downstream, not the modal.
        //
        // DELIBERATELY string-only: the consumer node that reads
        // this array decides whether to coerce to numbers,
        // booleans, paths, etc. If the modal coerced eagerly, a
        // user typing "42" for a downstream `string[]` consumer
        // would corrupt that consumer (because the array
        // contains a number, not a string). Decoupling keeps
        // the modal type-agnostic.
        if (!Array.isArray(raw)) return [];
        return raw.map((s) => (typeof s === 'string' ? s.trim() : ''))
          .filter((s) => s.length > 0);
      }
      case 'any':
      default: {
        const trimmed = typeof raw === 'string' ? raw.trim() : '';
        if (trimmed === '') return null;
        try {
          return JSON.parse(trimmed);
        } catch {
          return raw;
        }
      }
    }
  }

  function handleBackdrop(event: MouseEvent) {
    if (event.target === event.currentTarget) onCancel();
  }

  function handleKey(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      event.preventDefault();
      onCancel();
    }
  }
</script>

<svelte:window onkeydown={handleKey} />

<div class="backdrop" onclick={handleBackdrop} role="presentation">
  <div class="modal" role="dialog" aria-modal="true" aria-labelledby="inputs-title">
    <header>
      <h3 id="inputs-title">Inputs</h3>
      <p>
        Provide values for the {inputs.length} input{inputs.length === 1 ? '' : 's'}
        in this blueprint before running.
      </p>
    </header>

    <div class="body">
      {#if inputs.length === 0}
        <p class="muted">No inputs to provide.</p>
      {/if}
      {#each inputs as req (req.nodeId)}
        <div class="field">
          <div class="field-head">
            <span class="label" title={req.nodeId}>{req.label}</span>
            <code class="kind">{req.type}</code>
          </div>

          {#if req.type === 'file' || req.type === 'folder'}
            {@const isFolder = req.type === 'folder'}
            <div class="path-row">
              <input
                type="text"
                value={(values[req.name] as string) ?? ''}
                placeholder={isFolder ? 'C:\\path\\to\\folder' : 'C:\\path\\to\\file.txt'}
                oninput={(e) =>
                  (values = { ...values, [req.name]: e.currentTarget.value })}
              />
              <button
                class="btn browse-btn"
                type="button"
                onclick={() => browsePath(req)}
              >
                Browse…
              </button>
            </div>
            <div class="hint">
              {isFolder
                ? 'Pick a folder from the filesystem — or type the path manually.'
                : 'Pick a single file from the filesystem — or type the path manually.'}
            </div>
          {:else if req.type === 'multi-file'}
            {@const list = (values[req.name] as string[] | undefined) ?? []}
            <div class="multi-row">
              <div class="chip-list">
                {#each list as p, i (i)}
                  <span class="chip" title={p}>
                    <span class="chip-path">{p}</span>
                    <button
                      class="chip-remove"
                      type="button"
                      aria-label={`Remove ${p}`}
                      onclick={() => removeAt(req.name, i)}
                    >×</button>
                  </span>
                {/each}
                {#if list.length === 0}
                  <span class="empty-hint">No files picked yet.</span>
                {/if}
              </div>
              <button
                class="btn browse-btn"
                type="button"
                onclick={() => browsePath(req)}
              >
                + Add files
              </button>
            </div>
            <div class="hint">
              Each click opens a picker and APPENDS to the list. Click × on
              a chip to remove.
            </div>
          {:else if req.type === 'number'}
            <input
              type="number"
              step="0.1"
              value={(values[req.name] as string) ?? ''}
              oninput={(e) =>
                (values = { ...values, [req.name]: e.currentTarget.value })}
            />
          {:else if req.type === 'boolean'}
            <select
              value={(values[req.name] as string) ?? 'false'}
              onchange={(e) =>
                (values = { ...values, [req.name]: e.currentTarget.value })}
            >
              <option value="false">false</option>
              <option value="true">true</option>
            </select>
          {:else if req.type === 'string'}
            <input
              type="text"
              placeholder="string value"
              value={(values[req.name] as string) ?? ''}
              oninput={(e) =>
                (values = { ...values, [req.name]: e.currentTarget.value })}
            />
          {:else if req.type === 'array'}
            {#each (values[req.name] as string[] | undefined) ?? [''] as slot, i (i)}
              <div class="slot-row">
                <span class="slot-idx" aria-hidden="true">{i + 1}</span>
                <input
                  type="text"
                  placeholder={`Value ${i + 1}`}
                  value={slot}
                  oninput={(e) =>
                    updateArraySlot(req.name, i, e.currentTarget.value)}
                />
                <button
                  class="slot-remove"
                  type="button"
                  aria-label={`Remove slot ${i + 1}`}
                  title="Remove this slot"
                  onclick={() => removeAt(req.name, i)}
                >×</button>
              </div>
            {/each}
            <button
              class="btn slot-add"
              type="button"
              onclick={() => addArraySlot(req.name)}
            >
              + Add slot
            </button>
            <div class="hint">
              One array element per row. Empty rows are skipped when the
              blueprint runs so you can leave a trailing row blank while
              you decide whether to add more.
            </div>
          {:else}
            <!-- any: JSON textarea (legacy branch — preserved for
                 inputs whose declared type allows ANY JSON shape,
                 not the strict string-array case above). -->
            <textarea
              rows="2"
              spellcheck="false"
              placeholder="JSON value (or leave empty for null)"
              value={(values[req.name] as string) ?? ''}
              oninput={(e) =>
                (values = { ...values, [req.name]: e.currentTarget.value })}
            ></textarea>
            {#if parseErrors[req.name]}
              <div class="field-error">{parseErrors[req.name]}</div>
            {/if}
          {/if}
        </div>
      {/each}
    </div>

    <footer>
      <button class="btn" type="button" onclick={onCancel}>Cancel</button>
      <button
        class="btn btn-primary"
        type="button"
        onclick={() => {
          const parsed: Record<string, unknown> = {};
          for (const r of inputs) parsed[r.name] = parseFor(r, values[r.name]);
          // Snapshot the parsed values BEFORE handing off to the
          // caller so the saved history mirrors exactly what the
          // runner will consume. parseFor already trimmed, deduped,
          // and coerced the per-type values, so re-running the
          // same blueprint with this snapshot verbatim reproduces
          // the prior run. Cancel never reaches this click handler,
          // so we never dirty the history on cancel.
          if (storageKey) saveInputHistory(storageKey, parsed);
          onSubmit(parsed);
        }}
      >
        ▶ Run with inputs
      </button>
    </footer>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(15, 23, 42, 0.42);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 50;
    backdrop-filter: blur(1px);
    font-family: 'Inter', system-ui, sans-serif;
  }
  .modal {
    background: white;
    border-radius: 9px;
    box-shadow: 0 16px 40px rgba(0, 0, 0, 0.28);
    /* Slightly wider than the original 460px so picker rows with a
       Browse button + a long path don't cramp. */
    width: min(560px, 92vw);
    max-height: 80vh;
    display: flex;
    flex-direction: column;
    border: 1px solid #d4d4d8;
    overflow: hidden;
  }
  header {
    padding: 14px 18px 10px;
    border-bottom: 1px solid #e4e4e7;
  }
  header h3 {
    margin: 0;
    font-size: 14px;
    font-weight: 700;
    color: #18181b;
  }
  header p {
    margin: 4px 0 0;
    font-size: 11.5px;
    color: #71717a;
    line-height: 1.5;
  }
  .body {
    padding: 14px 18px;
    overflow-y: auto;
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .field {
    display: block;
  }
  .field-head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 8px;
    margin-bottom: 4px;
  }
  .field-head .label {
    font-size: 12px;
    font-weight: 600;
    color: #27272a;
    font-family: 'JetBrains Mono', monospace;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .field-head .kind {
    font-family: 'JetBrains Mono', monospace;
    font-size: 10.5px;
    background: #f4f4f5;
    border: 1px solid #e4e4e7;
    border-radius: 3px;
    padding: 1px 6px;
    color: #52525b;
  }
  .body input,
  .body textarea,
  .body select {
    width: 100%;
    border: 1px solid #d4d4d8;
    border-radius: 4px;
    padding: 6px 8px;
    font-size: 12.5px;
    background: white;
    color: #18181b;
    font-family: inherit;
    box-sizing: border-box;
  }
  textarea {
    font-family: 'JetBrains Mono', monospace;
    resize: vertical;
    min-height: 38px;
  }
  .body input:focus,
  .body textarea:focus,
  .body select:focus {
    outline: none;
    border-color: #2d7cf1;
    box-shadow: 0 0 0 2px rgba(45, 124, 241, 0.2);
  }
  footer {
    padding: 12px 18px;
    border-top: 1px solid #e4e4e7;
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    background: #fafafa;
  }
  .btn {
    height: 32px;
    padding: 0 14px;
    border: 1px solid #d4d4d8;
    background: white;
    border-radius: 5px;
    font-size: 12.5px;
    font-weight: 500;
    color: #27272a;
    cursor: pointer;
    font-family: inherit;
  }
  .btn:hover {
    background: #f4f4f5;
    border-color: #a1a1aa;
  }
  .btn-primary {
    background: #16a34a;
    border-color: #16a34a;
    color: white;
  }
  .btn-primary:hover {
    background: #15803d;
    border-color: #15803d;
  }

  /* Picker rows: text input + Browse button on one line. */
  .path-row,
  .multi-row {
    display: flex;
    gap: 6px;
    align-items: stretch;
  }
  .path-row input {
    flex: 1 1 0;
    min-width: 0;
    font-family: 'JetBrains Mono', monospace;
    font-size: 11.5px;
  }
  .multi-row {
    flex-direction: column;
    gap: 6px;
  }
  .browse-btn {
    flex: 0 0 auto;
    padding: 0 12px;
    border-color: #2d7cf1;
    color: #2d7cf1;
    background: white;
    font-weight: 500;
  }
  .browse-btn:hover {
    background: #2d7cf1;
    color: white;
  }
  .hint {
    font-size: 11px;
    color: #71717a;
    margin-top: 4px;
    line-height: 1.45;
  }

  /* Inline validation error: shown under JSON textareas (and any
     future row that needs to surface a parse failure) when the
     typed value does not match the declared input kind. Same
     line-height + margin-top as .hint so the visual rhythm
     doesn't break. Colour picks up the same red used by
     .run-panel .error. */
  .field-error {
    font-family: 'JetBrains Mono', monospace;
    font-size: 11px;
    color: #b91c1c;
    margin-top: 4px;
    line-height: 1.45;
  }

  /* Multi-file chip list. Each chip carries its own remove button. */
  .chip-list {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    padding: 6px 8px;
    border: 1px solid #d4d4d8;
    border-radius: 4px;
    background: #fafafa;
    min-height: 38px;
    align-items: center;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    background: white;
    border: 1px solid #d1d5db;
    border-radius: 3px;
    padding: 2px 2px 2px 8px;
    font-family: 'JetBrains Mono', monospace;
    font-size: 10.5px;
    color: #3f3f46;
    max-width: 100%;
  }
  .chip-path {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 280px;
    display: inline-block;
  }
  .chip-remove {
    background: transparent;
    border: none;
    color: #9ca3af;
    cursor: pointer;
    padding: 0 4px;
    font-size: 14px;
    line-height: 1;
    border-radius: 2px;
  }
  .chip-remove:hover {
    color: #ef4444;
    background: #fee2e2;
  }
  .empty-hint {
    font-size: 11px;
    color: #9ca3af;
    font-style: italic;
    padding: 0 4px;
  }
  .muted {
    color: #71717a;
    font-size: 12px;
    margin: 8px 0;
  }

  /* `array` slot list: one row per element with an index chip on
     the left so the user sees the slot ordering, a plain text
     input in the middle, and a compact × on the right. Stacks
     below the modal's standard input rhythm; the surrounding
     .field wrapper already gives top spacing. `gap: 6px` mirrors
     the modal's other stack rules so a row of slots reads as a
     single unit. */
  .slot-row {
    display: flex;
    align-items: stretch;
    gap: 6px;
    margin-top: 6px;
  }
  .slot-row:first-child {
    margin-top: 0;
  }
  .slot-idx {
    flex: 0 0 22px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: #f4f4f5;
    border: 1px solid #e4e4e7;
    border-radius: 4px;
    font-family: 'JetBrains Mono', monospace;
    font-size: 11px;
    font-weight: 600;
    color: #52525b;
    /* The size mirrors a single-line text input so the index chip
       sits flush vertically with the input next to it. */
    height: 32px;
  }
  .slot-row input {
    flex: 1 1 0;
    min-width: 0;
    font-family: 'JetBrains Mono', monospace;
    font-size: 11.5px;
  }
  .slot-remove {
    flex: 0 0 28px;
    height: 32px;
    /* Reuses the `.btn` shape for consistency but strips the
       outline so a row-of-buttons doesn't visually crowd. */
    border: 1px solid #d4d4d8;
    background: white;
    border-radius: 4px;
    font-size: 14px;
    line-height: 1;
    color: #9ca3af;
    cursor: pointer;
    padding: 0;
    font-family: inherit;
  }
  .slot-remove:hover {
    color: #ef4444;
    background: #fee2e2;
    border-color: #ef4444;
  }
  /* `+ Add slot` is the same `.btn` primitive as the Cancel /
     Run buttons but with a primary-coloured outline so a fresh
     user recognises it as the affirmative "more" action. */
  .slot-add {
    margin-top: 8px;
    border-color: #2d7cf1;
    color: #2d7cf1;
    background: white;
    font-weight: 500;
  }
  .slot-add:hover {
    background: #2d7cf1;
    color: white;
  }
</style>
