# Índice de documentación

Esta página lista cada documento y cada cabecera de sección de `docs/` para que un contributor pueda encontrar lo que necesita sin abrir los cuatro docs.

## Documentos

| Fichero | Audiencia | Líneas (aprox) |
|---------|-----------|----------------|
| [`ARCHITECTURE.md`](ARCHITECTURE.md) | Nuevos contributors, revisores | ~140 |
| [`BACKEND.md`](BACKEND.md) | Contributors Rust | ~500 |
| [`FRONTEND.md`](FRONTEND.md) | Contributors Svelte/TS | ~450 |
| [`MAINTAINING.md`](MAINTAINING.md) | Cualquiera que extienda la app | ~280 |

## Mapa de secciones

### `ARCHITECTURE.md`

- At a glance — diagrama ASCII de 5 capas.
- Cómo luce un blueprint en reposo — JSON de ejemplo.
- Flujo de datos en un Run — traza de 7 pasos por el sistema.
- Invariantes clave (léelas antes de cambiar nada):
  1. Un node kind, dos ficheros, tres registros.
  2. Las formas TS espejan las structs Rust.
  3. `schema_version` se incrementa con los cambios rompedores.
  4. Writables para el estado del grafo, `$state` para todo lo demás.
  5. `EvalCtx` es el único side-channel.
  6. Los puertos son declarativos pero el ejecutor tiene la autoridad.
- Decisiones de diseño y POR QUÉ (Tauri, Svelte Flow, serde_json Value, registro de function-pointer, helpers de broadcast, escritura atómica, migración de schema).
- Glosario.

### `BACKEND.md`

- Mapa de ficheros con conteos de líneas.
- `main.rs` — entry point.
- `lib.rs` — `BlueprintError`, `SaveResult`, `save_blueprint` / `load_blueprint` / `run_blueprint`, `write_atomically`, plugins.
- `blueprint.rs` — `SCHEMA_VERSION`, `Blueprint`, `BlueprintNode`, `Port`, `BlueprintConnection`.
- `executor/mod.rs` — `EvalCtx`, `LogEntry`, `NodeInputs`, `NodeOutputs`, `NodeExec`, `ExecError`, helpers.
- `executor/broadcast.rs` — `broadcast_numeric`, `broadcast_pairs`.
- `executor/runner.rs` — `run_graph`, `build_incoming`, `topo_sort`, `execute`, tests de integración.
- `executor/nodes/mod.rs` — registro + `test_helpers`.
- Executors por nodo — tablas at-a-glance por categoría.
- Tests — comandos y convención.

### `FRONTEND.md`

- Mapa de ficheros.
- `package.json`, `vite.config.js`, `svelte.config.js`, `tsconfig.json`.
- `src/app.html`, `+layout.ts`, `+page.svelte`.
- `src/lib/types/blueprint.ts` — tipos + `PortDataType` + colores + `makeMeta` + `stripMeta`.
- `src/lib/nodes/catalog.ts` — `CATEGORY_COLORS`, `ParamField`, `NodeDef`, `getDef`, `defaultParams`.
- `src/lib/utils/blueprint-io.ts` — `composeBlueprint`, `blueprintToFlow`, `makeFlowNode`, `edgeFromConnection`.
- `src/lib/utils/runner-io.ts` — `runBlueprint`, `renderValue`.
- `src/lib/components/Toolbar.svelte` — patrón snapshot vía `get(store)` de svelte/store, flujos save/load/new.
- `src/lib/components/Palette.svelte` — payload de drag + fallback click.
- `src/lib/components/Editor.svelte` — manejo de drop, `isValidConnection`, overrides `:global`.
- `src/lib/components/nodes/BlueprintNode.svelte` — un único componente para todos los kinds.
- `src/lib/components/Inspector.svelte` — modo dual metadata vs nodo.
- `src/lib/components/ResultsPanel.svelte`.
- Comandos de test.

### `MAINTAINING.md`

- Añadir un nuevo node kind — receta completa (Rust → registro → catálogo → verificación).
- Añadir un nuevo ParamField editable.
- Añadir un nuevo port type.
- Añadir un nuevo Tauri command.
- Añadir un source/destination de fichero al catálogo.
- Bumpear la versión del schema.
- Lanzar la suite de tests.
- Pitfalls comunes.

## Mapa de ficheros

| Path | Qué |
|------|-----|
| `README.md` | Quick-start a una pantalla + navegación; para visitantes nuevos |
| `docs/ARCHITECTURE.md` | La mitad "por qué" de la docs; diagrama de capas + invariantes + decisiones de diseño |
| `docs/BACKEND.md` | Recorrido del fuente Rust, struct por struct, helper por helper, inventario por nodo |
| `docs/FRONTEND.md` | Recorrido del fuente Svelte/TS, fichero por fichero, con cross-refs a los contratos TS↔Rust |
| `docs/MAINTAINING.md` | Cookbook de "cómo extiendo X" en orden de frecuencia |
| `docs/INDEX.md` | Esta página — navegación sección por sección |
