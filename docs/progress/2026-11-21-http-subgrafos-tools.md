---
type: progress
project: blueprint-processor
created: 2026-11-21
updated: 2026-11-21
status: completed
tags: [progress]
---

# HTTP, subgrafos y tools de IA

## Objective

Tres capacidades nuevas: llamar a un endpoint HTTP desde un nodo, incrustar
un blueprint guardado como un único nodo, y dar herramientas al nodo de IA
(tools globales de aplicación y tools definidas como subgrafo).

## Work Completed

- Nodo `http.request` (categoría IO) con `status` / `body` / `json`, método,
  URL, cabeceras y timeout. Transporte compartido con las app tools.
- Nodo `blueprint.subgraph`: puertos derivados del fichero incrustado
  (`metadata.inputs` + un output por `io.output`), ejecución anidada con
  límite de 8 niveles y poda de wires a puertos desaparecidos.
- App tools: catálogo global en `<app_data_dir>/tools.json`
  (`app_tools.rs`) + comandos `tools_list` / `tools_save` / `tools_remove`
  + diálogo **AI Tools** en la toolbar.
- Nodo `ai.tool`: descriptor `{ name, description, parameters,
  blueprint_path }` para cablear al nuevo puerto `Tools` de
  `ai.completion`.
- `ai.completion`: selección de app tools por id, bucle de tool-calling
  acotado (`Max tool rounds`), ejecución de app tools por HTTP y de tools
  de nodo por blueprint anidado. Un fallo de tool se devuelve al modelo.
- Provider tool-calling en `ai/providers.rs` con JSON directo para
  OpenAI-compatible y Anthropic; `AiGateway` gana `tools()` y
  `complete_turn()` con defaults no rompientes.
- Inspector: `ParamField` `blueprint` (selector de librería + ruta manual)
  y `tools` (checklist).
- Documentación: changelog 0.1.0, BACKEND, FRONTEND, FD-003/FD-004 y
  ADR-006.

## Current State & Verified Behavior

Comandos ejecutados y resultado real:

- `cd src-tauri && cargo check --all-targets` → exit 0 (solo warnings).
- `cd src-tauri && cargo test --lib` → `279 passed; 3 failed`. Los 3 fallos
  son los preexistentes de `executor::nodes::file_iterate_folder`
  (PROD-TECH-007), ajenos a esta entrega.
- Tests nuevos que pasan: `blueprint_subgraph` (run real de un blueprint
  embebido, límite de anidamiento, fichero ilegible, path ausente),
  `ai_completion::wired_tool_runs_the_subgraph_and_returns_the_final_answer`,
  `ai_completion::unknown_app_tool_errors`, `ai_tool` (descriptor y
  validación), `http_request` (cabeceras, URL vacía, esquema) y
  `app_tools` (CRUD, saneado, rechazo de tools inválidas).
- `pnpm check` → `0 errors and 3 warnings in 2 files`; los warnings son
  a11y preexistentes en `Palette.svelte` y `dashboard/+page.svelte`.

## Problems & Blockers

- El camino HTTP real (nodo y app tools) no se probó contra un endpoint
  vivo; los tests cubren parsing, validación y errores previos a la red.
- El bucle de tools se probó con `MockGateway` guionizado y un blueprint
  temporal en disco; no contra OpenAI/Anthropic reales.

## Next Steps

- Probar `http.request` y una app tool contra un endpoint local.
- Descubrimiento de esquema para app tools (p. ej. OpenAPI) si se necesita.
- Prefijar los logs de subgrafos con el id del nodo contenedor.
