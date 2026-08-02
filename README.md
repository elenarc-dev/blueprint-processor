# Blueprint Processor

Aplicación de escritorio para componer **blueprints** — pipelines visuales y modulares de procesamiento de datos — y ejecutarlos en un runtime Rust sandboxed. Modelado tras los Blueprints de Unreal y el grafo de shaders de Blender.

Compones un pipeline en un canvas:

```[dataset.csv] ──▶ [string.split · ","] ──▶ [math.multiply × 2] ──▶ [array.filter · ≥10] ──▶ [io.output · "result"] │
                                      [io.logger] ◀────────────────────────────────────── ┘
```

Luego pulsas **Run** y el ejecutor Rust evalúa el grafo en orden topológico determinista, devolviendo `{ outputs, logs, stats }`.

## Quick start

```bash
pnpm install
pnpm tauri dev      # app completa (frontend + shell Tauri)
# O bien
pnpm dev            # sólo frontend (sin save/run — para trabajo puramente UI)
cd src-tauri && cargo test --lib     # tests unitarios + de integración Rust
pnpm check                            # svelte-check / typecheck TS
```

Requisitos: Node 20+, pnpm, Rust stable, prerequisitos Tauri 2 para tu SO, WebView basado en Chromium (Windows trae uno de serie).

## Dónde leer a continuación

La documentación vive en `docs/`. Elige el punto de entrada que encaje con lo que quieres hacer.

| Qué quieres hacer | Lee |
|---|---|
| Entender el diseño general y el flujo de datos | [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) |
| Trabajar en el ejecutor Rust / añadir un nuevo node kind | [`docs/BACKEND.md`](docs/BACKEND.md) |
| Trabajar en la UI Svelte / añadir un nuevo componente | [`docs/FRONTEND.md`](docs/FRONTEND.md) |
| Añadir un nodo, port type, IPC command, o schema bump — cookbook | [`docs/MAINTAINING.md`](docs/MAINTAINING.md) |

## Layout del proyecto

```
.
├── docs/                     ← estás aquí en espíritu
│   ├── ARCHITECTURE.md
│   ├── BACKEND.md
│   ├── FRONTEND.md
│   ├── MAINTAINING.md
│   └── INDEX.md              ← mapa de secciones para grep
├── src/                      ← App SvelteKit (frontend)
│   ├── app.html
│   ├── routes/
│   │   ├── +layout.ts        ← ssr=false (Tauri no puede SSR)
│   │   ├── +layout.svelte
│   │   └── +page.svelte      ← shell principal, dueño del writable state
│   └── lib/
│       ├── components/       ← Toolbar, Palette, Editor, Inspector, ResultsPanel
│       │   └── nodes/        ← BlueprintNode + registro nodeTypes
│       ├── nodes/catalog.ts  ← node kinds canónicos que el usuario puede soltar
│       ├── types/blueprint.ts ← espejo TS de src-tauri/blueprint.rs
│       └── utils/
│           ├── blueprint-io.ts ← puente Svelte Flow ↔ JSON Blueprint
│           └── runner-io.ts    ← IPC Tauri (invoke 'run_blueprint')
├── src-tauri/                ← Backend Rust (shell Tauri)
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   ├── capabilities/default.json
│   └── src/
│       ├── main.rs           ← 6 LOC, llama a lib::run()
│       ├── lib.rs            ← 3 comandos Tauri + writer atómico
│       ├── blueprint.rs      ← esquema JSON en disco (v1)
│       └── executor/
│           ├── mod.rs        ← Value, EvalCtx, ExecError, helpers
│           ├── broadcast.rs  ← broadcasting scalar/array para ops binarias
│           ├── runner.rs     ← Kahn topo + ejecución por nodo
│           └── nodes/        ← UN FICHERO POR KIND (20 ficheros)
└── README.md                 ← este fichero
```

## Decisiones de diseño principales

- **Tauri** + **Svelte Flow** para el shell de escritorio.
- **JSON como lengua franca** entre capas — `serde_json::Value` en todo el lado Rust; `Record<string, unknown>` en todo el lado TS.
- **Un fichero de ejecución por node kind, en ambos lados** — añadir un nodo significa añadir dos ficheros (entrada de catálogo + módulo ejecutor) más una línea en cada registro.
- **Escritura atómica de ficheros** para que un crash durante un save nunca pueda corromper un fichero bueno anterior.
- **`schema_version` se bumpea sólo en cambios rompedores**; en otro caso cada nuevo campo opcional lleva `#[serde(default)]` y los ficheros viejos siguen funcionando.

Véase `docs/ARCHITECTURE.md` para la rationale en formato largo.

## Validación

```bash
pnpm check                                # typecheck TS / Svelte
cd src-tauri && cargo test --lib          # ~29 tests Rust
pnpm tauri dev                            # smoke manual
```

Un `pnpm check` exitoso es "sin errors y sin warnings" (hay un warning de a11y preexistente en `Palette` abierto y rastreado en el codebase). Un `cargo test` exitoso corre los tests unitarios por nodo más los dos tests de integración en `runner.rs` (`full_run_graph_literal_add_output` y `run_graph_detects_cycle`).
