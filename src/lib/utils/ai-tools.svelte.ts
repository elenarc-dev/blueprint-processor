// Module-level reactive state for the app-level AI tool catalog.
//
// Same pattern as `ai-config.svelte.ts`: tools are configured once
// (toolbar → AI Tools), stored in the app data directory, and read here
// by the Inspector so an `ai.completion` node can select them by id.
// Definitions never enter a blueprint.

import { invoke } from '@tauri-apps/api/core';

/** One app-level tool, as `tools_list` returns it (camelCase IPC). */
export interface AppTool {
  /** Stable handle stored in `params.app_tools`. */
  id: string;
  /** Function name the model calls. */
  name: string;
  description: string;
  /** JSON Schema object for the arguments. */
  parameters: unknown;
  method: string;
  url: string;
  /** Extra headers, one `Name: Value` per line. */
  headers: string;
}

export const aiTools = $state<{ value: AppTool[]; loaded: boolean }>({
  value: [],
  loaded: false,
});

export async function refreshAiTools(): Promise<AppTool[]> {
  try {
    aiTools.value = await invoke<AppTool[]>('tools_list');
  } catch {
    aiTools.value = [];
  }
  aiTools.loaded = true;
  return aiTools.value;
}

/** Load once per app session; components call this on mount. */
export function ensureAiTools(): void {
  if (!aiTools.loaded) void refreshAiTools();
}

/** Publish the list a save/remove command just returned. */
export function setAiTools(next: AppTool[]): void {
  aiTools.value = next;
  aiTools.loaded = true;
}
