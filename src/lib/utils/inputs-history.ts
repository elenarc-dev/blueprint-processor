/**
 * localStorage-backed history of the last SUBMITTED values for
 * `InputsDialog`, keyed per blueprint. The dialog wires
 * `storageKey` to a blueprint-unique string (Dashboard uses the
 * absolute file path; Editor uses `currentFilePath || bpMeta.id`)
 * so the same blueprint opens the dialog with the last submitted
 * values pre-filled.
 *
 * Best-effort storage: every entry point silently no-ops if the
 * browser environment is hostile (Tauri sandbox quirk, private
 * browsing mode, quota exceeded, JSON-corruption from a prior
 * version, SSR with no `window`, …). A failed read is treated the
 * same as no prior values — freshness is recovered next run.
 */

const KEY_PREFIX = 'bp-inputs:';

/**
 * Probe localStorage by writing + deleting a throwaway key. This
 * catches the cases where the API exists but is disabled (Safari
 * private-browsing, sandboxed iframes) where setItem throws a
 * `QuotaExceededError` immediately. Storing the probe under a
 * distinctive name means a stale probe can't collide with a real
 * blueprint key by accident.
 */
function isLocalStorageAvailable(): boolean {
  try {
    if (typeof window === 'undefined' || !window.localStorage) return false;
    const probe = '__bp_storage_probe__';
    window.localStorage.setItem(probe, '1');
    window.localStorage.removeItem(probe);
    return true;
  } catch {
    return false;
  }
}

/**
 * Merge shape: `{ [inputName]: parsedValue }` exactly as the
 * caller passed to `saveInputHistory` AND as `InputsDialog`
 * hands off to `onSubmit`. We do NOT validate the value against
 * the input's declared type here — the dialog does that on the
 * next render and falls back to per-type defaults for any
 * mismatch (see `coercePersistedIntoRaw` on the dialog side).
 */
export type InputHistoryPayload = Record<string, unknown>;

/**
 * Read the last submitted values for `storageKey`, or null if
 *   * no key was supplied
 *   * localStorage is unavailable
 *   * the entry is missing
 *   * the entry is corrupt (JSON.parse failure)
 *   * the entry isn't a plain object (someone hand-rolled a
 *     non-object into the slot)
 *
 * The return value is always safe to feed into a defensive merge
 * loop, even on a hostile read.
 */
export function loadInputHistory(
  storageKey: string | null | undefined,
): InputHistoryPayload | null {
  if (!storageKey) return null;
  if (!isLocalStorageAvailable()) return null;
  try {
    const raw = window.localStorage.getItem(KEY_PREFIX + storageKey);
    if (!raw) return null;
    const parsed: unknown = JSON.parse(raw);
    if (
      parsed !== null &&
      typeof parsed === 'object' &&
      !Array.isArray(parsed)
    ) {
      return parsed as InputHistoryPayload;
    }
    return null;
  } catch {
    return null;
  }
}

/**
 * Persist the parsed value map. Called by `InputsDialog` right
 * before invoking `onSubmit` so the snapshot that hits
 * localStorage matches what the runner actually consumed.
 *
 * Best-effort: catches every Storage error and swallows it.
 * Persistence is a UX feature, not a correctness feature — a
 * throwaway failure here must not abort a run the user has
 * already committed to.
 */
export function saveInputHistory(
  storageKey: string | null | undefined,
  values: InputHistoryPayload,
): void {
  if (!storageKey) return;
  if (!isLocalStorageAvailable()) return;
  try {
    window.localStorage.setItem(
      KEY_PREFIX + storageKey,
      JSON.stringify(values),
    );
  } catch {
    // Quota exceeded, sandboxed frame, etc. — see the JSDoc.
  }
}

/**
 * Drop the persisted entry. Currently unreferenced by the rest
 * of the codebase but exposed for a future "clear last run" UI
 * affordance or for tests that want to assert a clean state.
 */
export function clearInputHistory(
  storageKey: string | null | undefined,
): void {
  if (!storageKey) return;
  if (!isLocalStorageAvailable()) return;
  try {
    window.localStorage.removeItem(KEY_PREFIX + storageKey);
  } catch {
    // best-effort; mirror `saveInputHistory`'s swallow policy.
  }
}
