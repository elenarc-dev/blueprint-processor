# Arquitectura

Blueprint Processor es una aplicación de escritorio que permite al usuario componer **blueprints** — pipelines de procesamiento de datos visuales — uniendo nodos (estilo Blueprints de Unreal / grafo de shaders de Blender) y ejecutándolos en un ejecutor Rust sandboxed. El frontend es SvelteKit + Svelte Flow; el backend es Tauri + Rust.

En runtime colaboran tres capas, comunicándose por JSON sobre el bridge IPC de Tauri:

```
┌─────────────────────────────────────────────────────────────────────┐
│  UI Svelte   (src/lib/components/*.svelte + src/routes/+page.svelte)│
│  ┌────────┐ ┌────────┐ ┌────────────────┐ ┌────────────┐             │
│  │Toolbar │ │Palette │ │Editor (canvas) │ │Inspector   │             │
│  └────────┘ └────────┘ └────────────────┘ └────────────┘             │
│                  Grafo Svelte Flow en runtime                        │
│                  (FlowNode[] + FlowEdge[] como writables)           │
└──────────────┬───────────────────────────────────────────────────────┘
               │
       src/lib/utils/blueprint-io.ts
       (composeBlueprint / blueprintToFlow / makeFlowNode)
               │
       src/lib/utils/runner-io.ts
       (runBlueprint → invoke('run_blueprint', { json, inputs }))
               │
        IPC Tauri (JSON, request/response)
               │
┌──────────────▼───────────────────────────────────────────────────────┐
│  Backend Rust   (src-tauri/src)                                     │
│                                                                     │
│  lib.rs      3 #[tauri::command] fns: save/load/run_blueprint      │
│              escritura atómica de fichero, migración de schema      │
│  blueprint.rs Blueprint, BlueprintNode, BlueprintConnection,        │
│              BlueprintMetadata, Port — esquema en disco (v1)       │
│  executor                                                           │
│     mod.rs      Value (= serde_json::Value), ExecError, EvalCtx,    │
│                 tipo NodeExec, helpers compartidos (value_to_string,│
│                 single, empty_outputs, f64_to_value, value_type_name)│
│     broadcast.rs broadcast_numeric + broadcast_pairs                │
│                 (scalar × scalar, scalar × array, arrays de         │
│                  igual longitud)                                    │
│     runner.rs   Kahn topológico + dispatch por nodo vía el          │
│                 registro construido por nodes/mod.rs                │
│     nodes/      UN FICHERO POR KIND + registro en mod.rs            │
│        ─ 4 literal.*     ─ 5 math.*      ─ 4 string.*              │
│        ─ 4 array.*       ─ 3 io.* (input / output / logger)         │
└─────────────────────────────────────────────────────────────────────┘
```

## Cómo luce un blueprint en reposo

```json
{
  "schema_version": 1,
  "id": "uuid",
  "name": "greet",
  "description": "",
  "created_at": "2026-…",
  "updated_at": "2026-…",
  "nodes": [
    {
      "id": "n1",
      "kind": "literal.string",
      "category": "Literal",
      "position": { "x": 0, "y": 0 },
      "label": null,
      "params": { "value": "hello" },
      "inputs":  [],
      "outputs": [{ "id": "value", "name": "Value", "type": "string" }]
    },
    {
      "id": "n2",
      "kind": "io.output",
      "category": "IO",
      "position": { "x": 220, "y": 0 },
      "label": null,
      "params": { "name": "message" },
      "inputs":  [{ "id": "in", "name": "In", "type": "any" }],
      "outputs": []
    }
  ],
  "connections": [
    { "id": "e1", "sourceNode": "n1", "sourcePort": "value",
      "targetNode": "n2", "targetPort": "in" }
  ],
  "metadata": { "author": null, "tags": [] }
}
```

`schema_version: 1` es la única versión en producción hoy. La constante vive en `src-tauri/src/blueprint.rs: SCHEMA_VERSION`. Véase *Bumping the schema* en `docs/MAINTAINING.md` cuando introduzcas algo que rompa ficheros antiguos.

## Flujo de datos en un Run

1. El usuario pulsa **Run** en `src/lib/components/ResultsPanel.svelte`.
2. `+page.svelte: handleRun()` lee `flowNodes` / `flowEdges` (los dos mirrors `$state` de los writables de Svelte Flow), prepara un map `inputs: {}` (vacío en v1; se rellenará cuando se añada un formulario para `io.input`) y llama a `runBlueprint` desde `src/lib/utils/runner-io.ts`.
3. `runBlueprint` invoca `composeBlueprint(meta, …)` para producir un `Blueprint` completo, hace `JSON.stringify` y dispara `'run_blueprint'` con `{ json, inputs }`.
4. El comando Rust `run_blueprint` (`src-tauri/src/lib.rs`) parsea el JSON y llama a `run_graph(&bp, &inputs)` desde `executor/runner.rs`.
5. `run_graph` construye el índice de edges entrantes, hace Kahn sort, y luego itera el orden. Por cada nodo pide al registro en `executor/nodes/mod.rs` su `NodeExec` (un puntero a función), recoge inputs de outputs previos, ejecuta con sus `params`, y guarda outputs en un lookup `HashMap<(node_id, port_id), Value>`.
6. `io.output` escribe en `EvalCtx.outputs`, `io.logger` añade a `EvalCtx.logs`. Ambos llegan al `RunResult` final que vuelve por el bridge IPC.
7. El frontend guarda `result` en `let runResult = $state<RunResult | null>(null)` y `ResultsPanel` lo renderiza vía `renderValue()` (en `runner-io.ts`) con un cap de 24 elementos para arrays.

## Invariantes clave — léelas antes de cambiar nada

1. **Un kind de nodo, dos ficheros, tres registros.** Un nuevo kind `foo.bar` requiere:
   - `src-tauri/src/executor/nodes/foo_bar.rs` implementando `pub fn exec(...)` + un test unitario.
   - Una línea en `executor/nodes/mod.rs::setup_registry()` registrando el kind.
   - Una entrada en `src/lib/nodes/catalog.ts::NODE_CATALOG` declarando puertos, params y categoría.
   No hay que tocar el renderer — `BlueprintNode.svelte` lee el catálogo y renderiza todos los kinds a través de un único switch. (El catálogo es la fuente canónica del lado TS; el registro es la fuente canónica del lado Rust. No hay enforcement a tiempo de compilación de la consistencia entre lados — ten cuidado al añadir un kind.)
2. **Las formas TS en `src/lib/types/blueprint.ts` DEBEN espejar las structs Rust en `src-tauri/src/blueprint.rs`.** El `#[serde(rename_all = "camelCase")]` sobre `BlueprintConnection` es el puente. Añadir un campo es al menos un diff en 2 sitios (struct Rust + interface TS). Para nuevas variantes del `data_type` de un `Port`, añade también el tipo a `PortDataType` en TS y a `PORT_COLORS` (la paleta tipo Blender-shader-editor).
3. **`schema_version` se incrementa en cada cambio rompedor.** Hoy sólo `v1`. `lib.rs::save_blueprint` migra silenciosamente los ficheros legacy con `schema_version == 0`. Versiones más nuevas que las que la app conoce → `BlueprintError::UnsupportedVersion`.
4. **Writables para el grafo, `$state` para todo lo demás.** `nodes` / `edges` son writables de Svelte porque Svelte Flow los muta *in-place*. Todo lo observable fuera del canvas (Toolbar, Palette, Inspector, ResultsPanel, status bar) lee los mirrors `flowNodes` / `flowEdges` — los `$state` mantenidos vía `$effect(() => nodes.subscribe(v => (flowNodes = v)))` en `+page.svelte`. **No te suscribas dentro de componentes derivados** — lee el mirror `$state`.
5. **`EvalCtx` es el único side-channel.** Los executors son funciones puras de `(inputs, params, ctx)`. Inputs y outputs son datos; lo único mutable fuera del output vive en `EvalCtx`: `node_id`, `kind`, `external_inputs`, `outputs` (los outputs con nombre del run), `logs` (las líneas de log del run). Añadir capacidades de I/O a un nodo significa añadir un campo a `EvalCtx` y enhebrarlo en `runner.rs::execute`.
6. **Los puertos son declarativos en el catálogo pero la autoridad la tiene el ejecutor.** `BlueprintNode.inputs` / `outputs` se persisten (así un layout de puertos custom podría sobrevivir un cambio de código sin lookup en el registro), pero el ejecutor *sólo lee* esos campos para validar que el `node.inputs[].id` encaja con sus expectativas; la lista de puertos real viene de la entrada del catálogo del kind + `nodes/mod.rs`. No añadas código que ramifique por nombre de puerto en el ejecutor sin también pinearlo en el catálogo.

## Decisiones de diseño y POR QUÉ

- **Tauri, no Electron.** Binario más pequeño (usa WebView2 en Windows / WebKit en macOS / WebKitGTK en Linux), núcleo en Rust para que el ejecutor corra a velocidad nativa y no necesite cruzar dos bridges JS.
- **Svelte Flow, no un canvas custom.** Acotamos el código custom a `BlueprintNode.svelte`, edge-styling (`+page.svelte` y overrides `:global` sobre `.svelte-flow__node`), y `useSvelteFlow().deleteElements` para el botón de borrado por nodo. Pan/zoom/connect/select/delete vienen gratis.
- **`Value = serde_json::Value`.** Elegimos JSON dinámico sobre enum con tags porque los nodos tienen formas heterogéneas (arrays de objetos, strings fluyendo a operaciones de número tras coerción débil). El runner enhebra `Value` por todas partes; los ficheros de nodo convierten en el borde cuando necesitan un tipo Rust concreto.
- **`NodeExec = fn(...)`, no `trait Node`.** Un puntero a función es más barato que una vtable y hace del registro en `nodes/mod.rs` un literal de datos puro — fácil de grep, fácil de testear, sin `Box<dyn Trait>`. Cada nodo es un fichero independiente con su propio `#[cfg(test)] mod tests`.
- **Helpers de broadcast, no manejo de arrays por nodo.** `broadcast.rs` es la única fuente de verdad para "qué significa scalar × scalar / scalar × array / array de igual longitud × array de igual longitud". Casi todas las ops numéricas y de string sólo pasan una closure (p. ej. `|x, y| Ok(x + y)`) a `broadcast_numeric` (números) o `broadcast_pairs` (preservando `Value`). Añadir una nueva op binaria array-aware = reusar el helper.
- **Escritura atómica de ficheros.** `lib.rs::write_atomically` escribe primero a `.filename.<pid>.<nanos>.tmp`, luego hace rename. Un save con crash no puede corromper el fichero bueno anterior.
- **`schema_version: serde::default 0` se trata como legacy.** En save migramos silenciosamente `0 → SCHEMA_VERSION`. En load rechazamos ficheros de un schema más nuevo.

## Glosario

- **Blueprint** — Un documento `.blueprint.json` en disco. Versionado, dif-friendly.
- **Kind** — Identificador string de un tipo de nodo, p. ej. `math.add`. Espeja una ruta de módulo Rust.
- **Port** — Un socket de entrada o salida tipado en un nodo. `id` es machine-readable, `name` es sólo display, `type` ∈ `number | string | boolean | array | any`.
- **Node** — Una caja visual en el canvas. `(id, kind, params, position, inputs[], outputs[])`.
- **Edge / Connection** — Un cable entre dos puertos identificados por `(sourceNode, sourcePort, targetNode, targetPort)`.
- **External input** — Un valor pasado en tiempo de ejecución, keyed por el `params.name` de cualquier nodo `io.input` del grafo.
- **Node executor** — `NodeExec = fn(&NodeInputs, &Map<String, Value>, &mut EvalCtx) -> Result<NodeOutputs, ExecError>`. Pura salvo por side-effects vía `EvalCtx`.
- **Run result** — `{ outputs: HashMap<name, Value>, logs: Vec<LogEntry>, stats: RunStats }` devuelto por el IPC.
