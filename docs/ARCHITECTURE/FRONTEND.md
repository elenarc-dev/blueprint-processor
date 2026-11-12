# Frontend (Svelte + TypeScript)

El frontend es una app SvelteKit + Svelte 5 + TypeScript. El runtime es una única `+page.svelte` que monta cinco paneles (Toolbar / Palette / Editor / Inspector / ResultsPanel) más una status bar, usando Svelte Flow para el canvas.

## Mapa de ficheros

| Fichero | LoC | Por qué existe |
|---------|-----|----------------|
| `package.json` | ~25 | Scripts, deps. `@xyflow/svelte`, `@tauri-apps/api`, `@tauri-apps/plugin-dialog`. |
| `vite.config.js` | ~28 | Vite + SvelteKit + Tauri con puerto fijo (1420). |
| `svelte.config.js` | ~30 | `adapter-static` + fallback SPA para Tauri desktop. |
| `src/app.html` | 13 | Plantilla shell. |
| `src/routes/+layout.ts` | 5 | `ssr = false` — Tauri no puede hacer SSR. |
| `src/routes/+layout.svelte` | 7 | Pass-through `<slot/>`. |
| `src/routes/+page.svelte` | 242 | Página principal; monta los componentes, es dueña de los writables, los mirrors de estado, el handler de run y la status bar. |
| `src/lib/types/blueprint.ts` | 120 | TS canónico para la forma en disco + `PortDataType`, `PORT_COLORS`, `PORT_LABELS`, `makeMeta`, `stripMeta`. DEBE mantenerse en sync con `src-tauri/src/blueprint.rs`. |
| `src/lib/nodes/catalog.ts` | 302 | `NODE_CATALOG` — fuente única de verdad para qué puede colocar el usuario. Cada entrada es `kind, category, label, description, inputs[], outputs[], params[]`. |
| `src/lib/utils/blueprint-io.ts` | 143 | `composeBlueprint`, `blueprintToFlow`, `makeFlowNode`, `newEdgeId`, `edgeFromConnection`. El puente entre estado runtime de Svelte Flow y la forma `Blueprint` en disco. |
| `src/lib/utils/runner-io.ts` | 76 | `runBlueprint(meta, nodes, edges, inputs)` — invoca `'run_blueprint'` por IPC Tauri + helper `renderValue` para el panel de resultados. |
| `src/lib/components/Toolbar.svelte` | 266 | Barra superior. Botones New / Load / Save; invoca `'save_blueprint'` / `'load_blueprint'`. |
| `src/lib/components/Palette.svelte` | 172 | Barra lateral izquierda. Drag source para cada node kind; fallback click-to-spawn vía `CustomEvent`. |
| `src/lib/components/Editor.svelte` | 236 | El canvas. Envuelve `<SvelteFlow>`, dueño del drag/drop y de `isValidConnection`. |
| `src/lib/components/nodes/BlueprintNode.svelte` | 378 | Renderer por nodo. Un único componente para todos los kinds; renderiza header, body, handles y un editor inline de params + botón delete. |
| `src/lib/components/nodes/index.ts` | 12 | El map `nodeTypes` que consume Svelte Flow (hoy sólo `{ default: BlueprintNode }`). |
| `src/lib/components/Inspector.svelte` | 341 | Panel derecho. Modo dual: editor de metadata (sin selección), o editor de params de nodo (nodo seleccionado). Renderiza `number` / `string` / `textarea` / `boolean` / `select`. |
| `src/lib/components/ResultsPanel.svelte` | 240 | Panel inferior. Renderiza `RunResult.outputs`, `RunResult.logs`, y un botón Run. |
| `src/lib/components/AiKeysDialog.svelte` | ~250 | Diálogo de claves de proveedores IA (botón **AI Keys** en la toolbar). Llama a `ai_key_status` / `ai_set_key` / `ai_clear_key`. **Única** vía de entrada de una clave. |

## Build / configs de runtime

### `package.json`

Tres scripts:

- `dev` — corre `vite dev`. (Para iterar sólo-frontend sin Tauri.) El comando Tauri-aware es `tauri dev`.
- `build` — `vite build`. Lo usa `tauri build`.
- `check` — `svelte-kit sync && svelte-check --tsconfig ./tsconfig.json`. Lánzalo antes de commits para cazar regresiones de tipo / a11y.

Deps notables:
- `@xyflow/svelte` 0.1.39 — componente canvas.
- `@tauri-apps/api` (core) 2.x — `invoke()` desde el frontend.
- `@tauri-apps/plugin-dialog` 2.x — Diálogos Save / Open / ask.

### `svelte.config.js`

`@sveltejs/adapter-static` + la config de fallback SPA para que el `dist/index.html` estático sea lo que Tauri abre. Esto es obligatorio porque Tauri no puede alojar un server Node SSR.

### `vite.config.js`

Puerto fijo `1420` y setting strict-port para que la URL del dev Tauri se mantenga estable. HMR habilitado.

### `tsconfig.json`

Config TS estándar de SvelteKit. Extiende `.svelte-kit/tsconfig.json` (generado por `svelte-kit sync`).

## `src/app.html`

Shell mínimo con placeholders `%sveltekit.head%` / `%sveltekit.body%` y `data-sveltekit-preload-data="hover"` para que el hover-prefetch de links funcione (no lo usamos aún pero sale gratis y es forward-friendly).

## `src/routes/+layout.ts`

```ts
export const ssr = false;
```

**Por qué:** El WebView de Tauri no aloja Node — no hay runtime SSR. Apagar SSR convierte cada ruta en SPA, que es lo que queremos.

## `src/routes/+page.svelte`

El orquestador. Es dueño de:

- `nodes: Writable<FlowNode[]>` y `edges: Writable<FlowEdge[]>` — pasados a `<SvelteFlow>`. Svelte Flow los muta in-place.
- `bpMeta: $state<BlueprintMeta>` — metadata del blueprint (name, description, tags). NO está en el writable (no hace falta que el canvas lo sepa).
- `flowNodes: $state<FlowNode[]>` / `flowEdges: $state<FlowEdge[]>` — mirrors `$state` de los writables. Otros paneles leen estos vía `$derived` en lugar de tomar sus propias subscriptions.
- `runResult: $state<RunResult | null>`, `runError: $state<string | null>`, `running: $state(false)` — estado del run.
- `status: $state<{ message: string, kind: 'info' | 'error' }>` — mueve la status bar.

### Cableado de los mirrors (CRÍTICO)

```ts
$effect(() => nodes.subscribe((v) => (flowNodes = v)));
$effect(() => edges.subscribe((v) => (flowEdges = v)));
```

Cada panel lee el mirror `$state`, **nunca** se subscribe al writable directamente. **Por qué:** el writable no tiene subscribers reactivos — `$state` es lo que hace que la reactividad fine-grained de Svelte 5 funcione del lado consumidor.

### `handleRun`

```ts
async function handleRun() {
  if (running) return;
  running = true;
  runError = null;
  try {
    const inputs: Record<string, unknown> = {};
    const result = await runInRust(bpMeta, flowNodes, flowEdges, inputs);
    runResult = result;
    status = { message: `Run ok · ${stats.nodes_executed} nodes · ${stats.duration_ms} ms`, … };
  } catch (err) { … }
  finally { running = false; }
}
```

En v1 `inputs` está vacío. Trabajo futuro: recoger `params.name` de cada nodo `io.input` en `flowNodes` y mostrar un formulario antes de `handleRun`.

### Forma del layout

```html
<div class="app-shell">
  <Toolbar …/>
  <div class="main">
    <Palette/>
    <div class="canvas-wrap">
      <SvelteFlowProvider>
        <Editor {nodes} {edges} {existingEdges}/>
      </SvelteFlowProvider>
      {#if flowNodes.length === 0}<div class="empty-canvas">…</div>{/if}
    </div>
    <Inspector …/>
  </div>
  <ResultsPanel …/>
  <div class="status-bar">…</div>
</div>
```

`<SvelteFlowProvider>` es obligatorio porque el evento click-to-spawn de `Palette` aterriza en `Editor` que usa `useSvelteFlow()` — necesitan el mismo contexto.

`.empty-canvas` overlay el canvas con un mensaje "Arrastra un nodo desde la izquierda" cuando el grafo está vacío. Tiene `pointer-events: none` para que el drop target sobre `.canvas-host` siga recibiendo eventos drag de HTML5.

## `src/lib/types/blueprint.ts`

Forma TypeScript canónica del `Blueprint`. **Edita esto Y `src-tauri/src/blueprint.rs` a la vez** — no hay link a tiempo de compilación entre los dos.

### Exports

- `SCHEMA_VERSION = 1 as const` — bumpear ambas constantes TS y Rust.
- `PortDataType` — `'number' | 'string' | 'boolean' | 'array' | 'any'`.
- `Port` — `{ id, name, type: PortDataType }`.
- `Blueprint` — Forma JSON completa (encaja con `Blueprint` de Rust campo por campo, excepto que el campo de connections se llama `connections` haciendo match con `Vec<BlueprintConnection>`).
- `BlueprintConnection` — Encaja con la versión camelCase-wire en Rust. Los nombres son camelCase porque Rust tiene `#[serde(rename_all = "camelCase")]` y TS los lee tal cual.
- `BlueprintNode` — `{ id, kind, category?, position: {x,y}, label?, params, inputs, outputs }`.
- `BlueprintMeta` — El sobre de metadata que vive en `$state` aparte del grafo. El grafo mismo (FlowNode/FlowEdge) vive en el writable.
- `PORT_COLORS` / `PORT_LABELS` — Colores tipo Blender-shader-editor por tipo de puerto. Los usan tanto `BlueprintNode.svelte` como `Inspector.svelte`.
- `makeMeta(name)` — Genera un sobre de metadata fresco con nuevo UUID + timestamps. Se llama desde `Toolbar.svelte::handleNew`.
- `stripMeta(bp)` — Extrae el sobre de un Blueprint cargado.

## `src/lib/nodes/catalog.ts`

La fuente única de verdad para qué puede colocar el usuario en el canvas.

### Forma

```ts
export const CATEGORY_COLORS: Record<NodeCategory, string> = {
  Literal: '#a855f7', Math: '#06b6d4', String: '#22c55e',
  Array: '#ec4899', IO: '#f59e0b',
};

export type ParamField =
  | { kind: 'number';  key: string; label: string; default: number;  min?, max?, step? }
  | { kind: 'string';  key: string; label: string; default: string;  placeholder? }
  | { kind: 'boolean'; key: string; label: string; default: boolean; }
  | { kind: 'select';  key: string; label: string; default: string;  options: string[] };

export interface NodeDef {
  kind: string;
  category: NodeCategory;
  label: string;
  description: string;
  inputs:  Array<Omit<Port, 'type'> & { type: PortDataType }>;
  outputs: Array<Omit<Port, 'type'> & { type: PortDataType }>;
  params:  ParamField[];
}
```

### Helpers exportados

- `getDef(kind): NodeDef | undefined` — Lo usan `BlueprintNode.svelte`, `Inspector.svelte` y `blueprint-io.ts`.
- `defaultParams(def): Record<string, unknown>` — Rellena un map `params` con los defaults del catálogo para un nodo recién creado.

### Categorías

`CATEGORIES` es un array con las 5 categorías en orden de display. Añadir una categoría significa editar esta constante Y `CATEGORY_COLORS`.

## `src/lib/utils/blueprint-io.ts`

El puente entre el estado runtime de Svelte Flow y la forma `Blueprint` en disco.

### `composeBlueprint(meta, nodes, edges) -> Blueprint`

Levanta un snapshot de estado de Svelte Flow a un documento `Blueprint` completo. Mapea campos FlowNode → BlueprintNode, estampa `updated_at = new Date().toISOString()`, descarta edges sin source o target handle (un edge parcialmente formado de un drag a medio camino).

### `blueprintToFlow(bp) -> { nodes, edges }`

Inversa de `composeBlueprint`. Para cada `bpNode` consulta `getDef(kind)` para combinar `defaultParams` sobre los params persistidos (así un fichero cargado con una versión anterior aún tiene defaults sensatos, y un usuario revisitando un fichero viejo recibe defaults frescos para params nuevos).

### `makeFlowNode(kind, position) -> FlowNode`

Crea un nodo Svelte Flow fresco. UUID vía `crypto.randomUUID()` donde esté disponible; fallback a `math.random` para WebViews más viejas.

### `edgeFromConnection(conn) -> FlowEdge`

Envuelve un evento `Connection` de Svelte Flow en un `FlowEdge` con un id único (`e_<src>__<target>__<rand6>`).

## `src/lib/utils/runner-io.ts`

La capa IPC fina.

### `runBlueprint(meta, nodes, edges, inputs) -> Promise<RunResult>`

```ts
const { json } = buildPayload(meta, nodes, edges, inputs);
const result = await invoke<RunResult>('run_blueprint', { json, inputs });
return result;
```

Nota el segundo arg de `invoke`: `{ json, inputs }`. Las claves DEBEN ser camelCase para encajar con la signature del comando Tauri (Rust las levanta por nombre de parámetro; Tauri 2 normaliza args a snake_case en el wire automáticamente).

Los errores lanzados por Rust llegan como strings (porque la impl `Serialize` de `ExecError` los aplana). Los envolvemos en un `Error(String(err))` fresco para que el try/catch del consumidor siga funcionando.

### `renderValue(v: unknown) -> string`

Pretty-printer recursivo. Para que `RunResult.outputs.{name}` (que es un valor JSON arbitrario) se renderice razonablemente en el panel de resultados — strings, números, booleanos, `null/undefined → "null"`, arrays con cap de 24 elementos (`[…, +N more]`), objetos vía `JSON.stringify`.

## `src/lib/components/Toolbar.svelte`

Barra superior con Save / Load / New + campo de nombre.

### Patrón snapshot (CRÍTICO)

```ts
const payload: Blueprint = composeBlueprint(meta, get(nodes), get(edges));
```

`get(...)` desde `svelte/store` lee el valor actual **sin** tomar una subscription. Sólo necesitamos un snapshot por click; no queremos que Toolbar se re-renderice cada vez que el canvas mute.

### Flujo Save

1. Abre el diálogo Save del SO (`saveDialog(...)` desde `@tauri-apps/plugin-dialog`). El usuario elige el path.
2. Construye el payload `Blueprint`, lo `JSON.stringify` con pretty-print.
3. `invoke('save_blueprint', { json, path })` → Rust valida contra la struct `Blueprint`, migra legacy `schema_version == 0`, pretty-print y atomic-rename sobre el path. Devuelve `{ path, bytes }`.
4. La status bar muestra el tamaño del save + path.

### Flujo Load

1. Abre el diálogo Open del SO.
2. `invoke('load_blueprint', { path })` → Rust devuelve el string JSON normalizado.
3. `JSON.parse` + `blueprintToFlow(...)` + `nodes.set(...)` / `edges.set(...)`.
4. Actualiza `bpMeta` vía `onMetaChange` (no mutación directa de la prop `meta`).

### Flujo New

Si el grafo no está vacío, pregunta vía diálogo ask del SO (`ask({ kind: 'warning' })`). Si confirma, limpia `nodes`/`edges` y siembra un `makeMeta('Untitled Blueprint')` fresco.

## `src/lib/components/Palette.svelte`

Barra lateral izquierda. Lista cada `NodeDef` del catálogo agrupado por categoría.

### Payload de drag

```ts
function onDragStart(event: DragEvent, kind: string) {
  event.dataTransfer.setData('application/x-blueprint-node', JSON.stringify({ kind }));
  event.dataTransfer.effectAllowed = 'copy';
}
```

El mime type `application/x-blueprint-node` es bespoke de esta app. `Editor.svelte::onDrop` lo lee de vuelta.

### Fallback click-to-spawn

HTML5 drag-and-drop es poco fiable dentro del WebView de Tauri (el cursor OS del drop a veces se queda en estado 'no-drop'). Como fallback, el `onclick` sobre un item del palette dispara un `CustomEvent('blueprint:add-node', { detail: { kind } })` a nivel window. `Editor.svelte::onMount` escucha ese evento y crea el nodo en el centro del canvas visible.

### Categorías

Se itera `CATEGORIES` en orden; cada grupo es un `<section>` con un `<h4>` cuyo color viene de `CATEGORY_COLORS`. Los items llevan el pequeño dot indicador del mismo color.

## `src/lib/components/Editor.svelte`

Envuelve `<SvelteFlow>`. Es dueño del manejo de drop, las reglas de conexión válida, y la integración visual.

### Drag & drop

```ts
function onDragEnter(event) { event.preventDefault(); event.dataTransfer.dropEffect = 'copy'; }
function onDragOver(event)  { event.preventDefault(); event.dataTransfer.dropEffect = 'copy'; }
function onDrop(event) {
  event.preventDefault();
  const payload = event.dataTransfer.getData('application/x-blueprint-node');
  const { kind } = JSON.parse(payload);
  const flowPos = screenToFlowPosition({ x: event.clientX, y: event.clientY });
  nodes.update(prev => [...prev, makeFlowNode(kind, flowPos)]);
}
```

**¿Por qué tres handlers?** WebView2/Chromium requiere que TANTO `dragenter` COMO `dragover` llamen a `preventDefault` antes de que el cursor OS del drop cambie de "no-drop" a "copy". Si te saltas `dragenter`, el primer intento de drop falla.

### `isValidConnection`

Rechaza auto-conexiones (`source === target`), conexiones sin handle, y duplicados sobre el mismo `(targetNode, targetPort)`. El check de duplicado usa `existingEdges`, computado en `+page.svelte` como un map derivado de los writables.

### Integración visual

El bloque `<style>` declara un override `:global(.canvas-host .svelte-flow)` para neutralizar los defaults de Svelte Flow v0.1.39 que duplican nuestro propio border / box-shadow / hover-shadow / background del wrapper. La lista de overrides incluye `--xy-node-border-default: none`, `--xy-node-background-color-default: transparent`, `--xy-node-boxshadow-{hover,selected}-default: none`, más una regla `:global(.canvas-host .svelte-flow__node) { background: transparent !important; border: none !important; … }` como belt-and-suspenders contra quirks de re-inyección HMR. **No los quites** — véase el commit "Refactor: belt-and-suspenders wrapper override" para la saga.

## `src/lib/components/nodes/BlueprintNode.svelte`

El único componente que renderiza cada node kind. Svelte Flow lo recibe vía `nodeTypes = { default: BlueprintNode }` (registrado en `nodes/index.ts`).

### ¿Por qué un único componente?

Añadir un nodo nuevo NO debe requerir un nuevo componente Svelte. El catálogo `NodeDef` ya codifica inputs/outputs; este componente los consume. El editor inline de params en el body usa una escalera `{#if d.kind === 'literal.number'}` — añadir un nuevo tipo de param editable es una nueva rama `{:else if}`.

### Botón de delete por nodo

```ts
const { updateNodeData, deleteElements } = useSvelteFlow();
function deleteThis(event: MouseEvent) {
  event.stopPropagation();   // no togglear selección
  deleteElements({ nodes: [{ id }] });
}
```

`useSvelteFlow` es llamable porque `BlueprintNode` se renderiza dentro de `<SvelteFlow>` (la página padre `+page.svelte` envuelve el editor en `<SvelteFlowProvider>`). `deleteElements` también elimina edges que tocan este nodo.

### Visual

- Banda header coloreada por `CATEGORY_COLORS[def.category]`. Texto blanco + soft text-shadow para legibilidad.
- Botón delete en la esquina superior derecha del header. × blanco semi-transparente → rojo al hover.
- Layout de filas: handles de input (izquierda) + etiqueta, editor inline opcional, handles de output (derecha) + etiqueta.
- Los handles se categorizan por color de port type (`PORT_COLORS`) y forma (number→círculo, string→cuadrado, boolean→diamante, array→triángulo, any→círculo hueco). Las formas viven en las reglas `:global(.svelte-flow__handle.port-*)` al final del fichero.

### ¿Por qué `overflow: visible` en `.node`?

Svelte Flow v0.1.39 posiciona los handles con offsets negativos FUERA del bounding box del nodo. `overflow: hidden` los recorta — el usuario ve bordes planos en el límite del nodo.

## `src/lib/components/nodes/index.ts`

Tan sólo `export const nodeTypes = { default: BlueprintNode } as const` y re-exporta el wrapper. La prop `nodeTypes` de Svelte Flow toma un `Record<string, Component>`. Tenemos un "type" (el renderer default) usado para cada kind; el switch del kind ocurre DENTRO de `BlueprintNode.svelte`.

## `src/lib/components/Inspector.svelte`

Panel derecho. Dos modos:

- **Sin selección** — surface de `bpMeta` (name, description, schema actual, ID) + el `NODE_CATALOG` completo enumerado como referencia.
- **Nodo seleccionado** — muestra el id del nodo seleccionado, kind, editor de label, y un editor de params dirigido por `def.params` (la misma escalera `ParamField` que el editor inline de `BlueprintNode`).

En cada edición, se llama `onNodeSave(nodeId, patch)`. `+page.svelte::handleNodeSave` aplica el patch re-mapeando el nodo bajo el mismo id en el writable.

`Inspector` lee `nodes` (el mirror del writable). **No** añadas una subscription directa al writable aquí — lee la prop.

## `src/lib/components/ResultsPanel.svelte`

Panel inferior.

- Idle (sin resultado): un botón Run + un hint sobre nombres de inputs.
- Running: botón deshabilitado + texto spinner.
- Resultado presente: una tabla de `outputs` renderizada vía `renderValue()` y una lista de logs (`logs: LogEntry[]`). Cap en arrays muy largos (`renderValue` muestra `+N more`).
- Error: banner rojo con el string de `ExecError::Display`.

## Comandos de test

```bash
pnpm install
pnpm check          # svelte-check en todo el workspace
pnpm tauri dev      # dev completo (frontend + shell Tauri)
cd src-tauri && cargo test --lib
```

No hay tests del lado Svelte hoy. El frontend se verifica por el loop de dev + `pnpm check`. Si añadimos `vitest` más adelante, la seam natural es `blueprint-io.ts` + `runner-io.ts` porque ambas son utilities puras sin dependencias de componentes.
