---
type: progress
project: blueprint-processor
created: 2026-11-21
updated: 2026-11-21
status: completed
tags: [progress]
---

# Edición anidada de subgrafos y vuelta al padre

## Objective

Abrir el blueprint embebido en un nodo `blueprint.subgraph` o `ai.tool` en
el mismo canvas, mostrando la relación de anidamiento con el grafo actual, y
poder volver al padre sin perder su estado.

## Work Completed

- `graphStack` en `+page.svelte`: snapshot de cada ancestro (meta, nodos,
  edges, path) al abrir un subgrafo; `navigateTo(depth)` restaura un nivel y
  descarta los inferiores.
- Botón **Open embedded blueprint ↗** en el cuerpo de los nodos
  `blueprint.subgraph` y `ai.tool`, más otro en el Inspector para el param
  `blueprint`. Ambos acaban en `openSubgraph(path)`.
- `GraphBreadcrumb.svelte`: barra `Nested · raíz › sub › sub-sub`, ancestros
  clicables y **← Parent graph**. Solo aparece con profundidad > 0.
- Al volver: `invalidateSubgraph(childPath)` + limpieza de `subgraphPath` en
  los nodos contenedores restaurados, para re-derivar puertos desde el
  fichero editado.
- `Toolbar` recibe `onResetNesting`: Load y New limpian la pila.
- Aviso de cambios sin guardar al volver: `fingerprint()` serializa el
  nivel con `composeBlueprint` (sin `updated_at`), `settleLevel()` toma la
  baseline después de resolver los subgrafos y `Save` la rearma vía
  `onSaved`. Si difiere, `ask` del plugin de diálogos pide confirmación
  antes de restaurar el padre.
- Corregida la migración del historial de inputs: exige que `bpMeta.id` no
  cambie, así abrir un nivel no copia los inputs del padre al hijo.
- Docs: changelog, FRONTEND y FD-006.

## Current State & Verified Behavior

- `pnpm check` → `0 errors and 3 warnings in 2 files`; los warnings son
  a11y preexistentes en `Palette.svelte` y `dashboard/+page.svelte`.
- No se ejecutó `cargo test` porque esta entrega no toca Rust.
- No se hizo prueba visual/interactiva en navegador: el flujo de abrir,
  navegar migas y volver está verificado por tipos/compilación, no por
  interacción real. El diálogo nativo de confirmación (`ask`) no se probó
  fuera del runtime de Tauri.

## Problems & Blockers

- Cambiar de nivel mientras corre un run está bloqueado por un guard, pero
  un run lanzado desde un nivel y terminado tras navegar no se cancela.

## Next Steps

- Recordar el viewport (zoom/pan) de cada nivel al restaurarlo.
- Test de interacción (Playwright) para abrir → migas → volver.
