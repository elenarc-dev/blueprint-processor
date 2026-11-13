// Module-level reactive state for the global AI provider catalog.
//
// Providers (kind, endpoint, model list, API key) are configured once,
// in the toolbar's AI Providers dialog, and are never node params — see
// src-tauri/src/ai/secrets.rs for the storage side. This module is the
// frontend mirror of that catalog so the Inspector's Model dropdown can
// offer `providerId::model` choices without every component invoking the
// backend on its own.
//
// Same pattern as `blueprint-context.svelte.ts`: one exported `$state`
// read reactively from several components (Inspector, AiProvidersDialog).
//
// `hint` is the backend's last-4-chars preview; the full API key never
// crosses the IPC bridge (AI-SEC-002).

import { invoke } from '@tauri-apps/api/core';

/** One configured provider, as `ai_providers` returns it. */
export interface ProviderStatus {
  /** Stable handle a node refers to (`ollama::llama3.1:70b`). */
  id: string;
  kind: string;
  label: string;
  baseUrl: string;
  models: string[];
  configured: boolean;
  hint: string | null;
}

/** Draft shape `ai_save_provider` accepts (camelCase — Tauri v2 IPC). */
export interface ProviderDraft {
  /** Empty asks the backend to generate one from the kind (`openai`, `openai-2`, …). */
  id: string;
  kind: string;
  label: string;
  baseUrl: string;
  models: string[];
  /** Empty means "keep the stored key". */
  apiKey: string;
}

export const PROVIDER_KINDS = ['openai', 'anthropic'] as const;

/** Human label per kind; unknown kinds fall back to the raw string. */
export const KIND_LABELS: Record<string, string> = {
  openai: 'OpenAI-compatible (OpenAI, Groq, OpenRouter, Ollama…)',
  anthropic: 'Anthropic',
};

export const aiProviders = $state<{ value: ProviderStatus[]; loaded: boolean }>({
  value: [],
  loaded: false,
});

/** Display name for a provider: `label`, or `id` when there is no label. */
export function providerName(p: ProviderStatus): string {
  return p.label.trim() || p.id;
}

/**
 * Re-read the catalog from the backend. Outside a Tauri build (plain
 * `pnpm dev` in a browser) `invoke` rejects — the catalog simply stays
 * empty, which is also the "nothing configured yet" state the backend
 * falls back on for legacy blueprints.
 */
export async function refreshAiProviders(): Promise<ProviderStatus[]> {
  try {
    aiProviders.value = await invoke<ProviderStatus[]>('ai_providers');
  } catch {
    aiProviders.value = [];
  }
  aiProviders.loaded = true;
  return aiProviders.value;
}

/** Load once per app session; components call this on mount. */
export function ensureAiProviders(): void {
  if (!aiProviders.loaded) void refreshAiProviders();
}

/**
 * Publish a list the backend just returned (every provider command answers
 * the full resulting catalog) so the Inspector's dropdown sees it without a
 * second round-trip.
 */
export function setAiProviders(next: ProviderStatus[]): void {
  aiProviders.value = next;
  aiProviders.loaded = true;
}
