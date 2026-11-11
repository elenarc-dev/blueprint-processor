---
type: progress
project: blueprint-processor
created: 2026-11-11
updated: 2026-11-11
status: completed
tags: [progress]
---

# Integración de IA (nodo `ai.completion`)

## Objective

Completar el bloque de tareas IA de `docs/TASKS.md` (AI-ARCH-001,
AI-SEC-002, AI-BACK-003, AI-BACK-004, AI-FRONT-005, AI-TEST-006)
integrando proveedores externos mediante `rig-core`, sin exponer
credenciales en los blueprints.

## Work Completed

- **Backend**
  - `src-tauri/src/ai/mod.rs`: `AiProviderKind`, `AiRequest`, `AiError`,
    `AiGateway` (dyn-safe, futuro boxeado), `AiEnv`, `RigGateway`,
    `UnconfiguredGateway`.
  - `src-tauri/src/ai/secrets.rs`: `SecretStore` sobre
    `<app_data_dir>/secrets.json`, escritura atómica, `0600` en Unix,
    `status()` sin filtrar claves.
  - `src-tauri/src/ai/providers.rs`: traducción a Rig (OpenAI
    Chat Completions con `base_url`, Anthropic Messages), reintentos
    acotados y extracción de bloques de texto.
  - `src-tauri/src/ai/mock.rs`: `MockGateway` para tests offline.
  - `executor/mod.rs`: `AsyncNodeExec`; `EvalCtx` **no** cambió.
  - `executor/runner.rs`: `run_graph`/`execute` async,
    `run_graph_with` con gateway inyectable, rama async que clona los
    inputs y suelta el mapa prestado antes del `await`.
  - `executor/nodes/ai_completion.rs`: executor async con espejo de
    forma y `buffered(concurrency)` para preservar el orden.
  - `lib.rs`: `run_blueprint` async + `ai_key_status`/`ai_set_key`/
    `ai_clear_key`.
- **Frontend**
  - `catalog.ts`: categoría `AI`, color y entrada `ai.completion`.
  - `Inspector.svelte`: `ParamField` tipo `textarea`.
  - `AiKeysDialog.svelte` (nuevo) + botón **AI Keys** en `Toolbar.svelte`.
- **Docs**: `ARCHITECTURE/decisions.md` (ADR-001…004), changelog 0.1.0,
  `TASKS.md` sin el bloque IA y con PROD-TECH-007 registrada.

## Current State & Verified Behavior

Comandos ejecutados y resultado real:

```bash
cd src-tauri && cargo test --lib
# 232 passed; 3 failed
# Fallos: file_iterate_folder::{reads_empty_file, reads_text_and_lines,
# handles_crlf_line_endings} -> "Not a directory (os error 20)"

cd src-tauri && cargo test --lib -- --skip file_iterate_folder
# test result: ok. 230 passed; 0 failed; 5 filtered out

pnpm check
# svelte-check found 0 errors and 3 warnings in 2 files
```

Los 3 fallos son **preexistentes**: `file_iterate_folder.rs` no aparece
modificado en `git status`, y sus tests pasan una ruta de fichero a
`fs::read_dir`. Los 3 warnings de `svelte-check` están en
`Palette.svelte` y `routes/dashboard/+page.svelte`, no en los ficheros
nuevos (0 warnings en `AiKeysDialog.svelte`, `catalog.ts`,
`Inspector.svelte` ni `Toolbar.svelte`).

Comprobado por los tests: el nodo IA propaga su salida a un nodo
funcional (`string.split` → `array.length`), el espejo de forma
array→array conserva orden, el fallback `params.prompt` funciona, y un
proveedor desconocido falla antes de llamar fuera.

**No verificado**: ninguna llamada real a OpenAI/Anthropic (no hay
claves en este entorno, y los tests no deben depender de red). La
integración HTTP con Rig compila y sus tipos cuadran, pero el camino
real de red queda pendiente de una prueba manual con clave.

## Problems & Blockers

- Sin claves de proveedor no se puede validar una completion real.
- `rig-core` es 0.46 (pre-1.0): está fijado con `=` y aislado en un
  fichero para que un bump no se propague.
- `docs/` tenía una reestructuración en curso sin commitear (borrado de
  `docs/BACKEND.md`, `docs/FRONTEND.md`, `docs/INDEX.md`,
  `docs/MAINTAINING.md` y nuevos `SPEC.md`, `WORKFLOW.md`,
  `ARCHITECTURE/`). Las ediciones de documentación de esta tarea se
  hicieron encima y **no** se incluyeron en el commit para no arrastrar
  cambios ajenos.

## Next Steps

1. Prueba manual con una clave real de OpenAI y de Anthropic
   (`pnpm tauri dev` → AI Keys → Run).
2. Plantillas de prompt con variables y salida estructurada (JSON
   schema), que es el caso de uso natural para procesar datasets.
3. Cancelación de runs en curso (hoy un proveedor lento bloquea el
   grafo hasta el timeout de red).
4. PROD-TECH-007: arreglar los tests de `file.iterate`.
