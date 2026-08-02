<!--
  Root layout. Two anchors:
   * Editor  →  `/`
   * Dashboard → `/dashboard`
  Both routes render their content under `<main>` via `{@render children?.()}`.
  The nav lives at the top-right of the viewport (fixed) so it overlays
  whatever route is active.
-->
<script lang="ts">
  import '../app.css';
  import { page as pageStore } from '$app/stores';

  let { children } = $props();

  const navPath = $derived($pageStore.url.pathname);
  // Highlight the Editor link when we're on `/` OR a deep-link like
  // `/?path=...` (the SvelteKit pathname is `/` either way, so this is
  // mostly defensive — the path itself covers both).
  const isEditor = $derived(navPath === '/' || navPath.startsWith('/?'));
  const isDashboard = $derived(navPath.startsWith('/dashboard'));
</script>

<nav class="top-nav" aria-label="Main">
  <span class="brand" aria-hidden="true">◇</span>
  <a href="/" class:active={isEditor} title="Blueprint editor">Editor</a>
  <a href="/dashboard" class:active={isDashboard} title="Running library">Dashboard</a>
</nav>

<main class="content">
  {@render children?.()}
</main>

<style>
  .top-nav {
    position: fixed;
    top: 10px;
    right: 10px;
    z-index: 100;
    display: flex;
    align-items: center;
    gap: 2px;
    background: rgba(255, 255, 255, 0.92);
    border: 1px solid #d4d4d8;
    border-radius: 7px;
    padding: 4px 6px;
    font-family: 'Inter', system-ui, sans-serif;
    font-size: 12.5px;
    box-shadow: 0 2px 8px rgba(0, 0, 0, 0.06);
    backdrop-filter: blur(6px);
  }
  .top-nav .brand {
    color: #2d7cf1;
    font-size: 14px;
    margin: 0 4px 0 2px;
    line-height: 1;
  }
  .top-nav a {
    color: #52525b;
    text-decoration: none;
    padding: 4px 10px;
    border-radius: 4px;
    font-weight: 500;
    transition:
      background 0.1s ease,
      color 0.1s ease;
  }
  .top-nav a:hover {
    background: #f4f4f5;
    color: #18181b;
  }
  .top-nav a.active {
    background: #2d7cf1;
    color: white;
  }

  /* Reserve room at the top so the fixed nav doesn't cover content. */
  .content {
    display: block;
    height: 100%;
    width: 100%;
    padding-top: 0;
  }
</style>
