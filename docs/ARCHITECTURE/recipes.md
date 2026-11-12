# Recetas de mantenimiento (Blueprint Processor)

Un cookbook práctico para extender la aplicación. Cada receta lista exactamente qué ficheros tocar y en qué orden.

## Añadir un nuevo node kind

Receta — añadiendo `string.uppercase` (string → mayúsculas) como ejemplo.

### 1. Ejecutor Rust

Crea `src-tauri/src/executor/nodes/string_uppercase.rs`:

```rust
//! `string.uppercase` — devuelve la forma en mayúsculas del input string.

use serde_json::{Map, Value};

use crate::executor::{
    single, value_to_string, EvalCtx, ExecError, NodeInputs, NodeOutputs,
};

pub fn exec(
    inputs: &NodeInputs<'_>,
    _params: &Map<String, Value>,
    _ctx: &mut EvalCtx<'_>,
) -> Result<NodeOutputs, ExecError> {
    let null = Value::Null;
    let raw = inputs.get("in").copied().unwrap_or(&null);
    Ok(single("out", Value::String(value_to_string(&raw).to_uppercase())))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use serde_json::{json, Value};
    use super::exec;
    use crate::executor::nodes::test_helpers::ctx_with;

    #[test]
    fn uppercases_ascii() {
        let params = json!({}).as_object().unwrap().clone();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let in_val = Value::String("hello".into());
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), &in_val);
        let mut ctx = ctx_with("n1", "string.uppercase", &external, &mut outs, &mut logs);
        let r = exec(&inputs, &params, &mut ctx).unwrap();
        assert_eq!(r.get("out").unwrap().as_str(), Some("HELLO"));
    }
}
```

Convenciones a seguir:
- La signature de la función es fija (véase `executor/mod.rs::NodeExec`).
- `let null = Value::Null; let v = inputs.get("in").copied().unwrap_or(&null);` es el patrón para defaultear inputs desconectados a `Null`. No uses `unwrap_or(&Value::Null::…)` — `Value::Null` no tiene un lifetime `&` útil.
- Un test positivo por camino de código. Usa `ctx_with` desde `nodes::test_helpers`.

### 2. Registrar el ejecutor

En `src-tauri/src/executor/nodes/mod.rs`:

```rust
pub mod string_uppercase;          // añadir al grupo de declaraciones de mod

// …dentro de setup_registry():
r.insert("string.uppercase".into(), string_uppercase::exec as NodeExec);
```

Coloca la entrada bajo el comentario de la categoría que encaje ("String" en este caso).

Lanza `cd src-tauri && cargo test --lib string_uppercase` para confirmar.

### 3. Entrada del catálogo

En `src/lib/nodes/catalog.ts`, append:

```ts
{
  kind: 'string.uppercase',
  category: 'String',
  label: 'Uppercase',
  description: 'Devuelve el input string en mayúsculas.',
  inputs:  [{ id: 'in', name: 'In', type: T.string }],
  outputs: [{ id: 'out', name: 'Out', type: T.string }],
  params:  [],
},
```

Fíjate en tres cosas:
- `category` debe encajar con una entrada de `NodeCategory` (y tener un color en `CATEGORY_COLORS`).
- El `T.string` es la constante helper al principio del fichero (`{ number: 'number', … }`) para que el literal `'string'` no se repita.
- No hay que tocar `BlueprintNode.svelte` — el editor inline de un nodo sin `params` simplemente no renderiza la fila de editor.

### 4. Verificar end-to-end

```bash
pnpm tauri dev
# suelta un literal.string → string.uppercase → io.output en el canvas
# ☑  el palette muestra "Uppercase"
# ☑  el nodo se renderiza con header verde
# ☑  introduce "hi" → run → el output es "HI"
```

## Añadir un nuevo ParamField editable

Los ParamField son los editores inline en `BlueprintNode.svelte` y el editor de formulario en `Inspector.svelte`. Hoy tenemos `number | string | boolean | select`. Para añadir p. ej. `multiline-string`:

1. En `src/lib/nodes/catalog.ts`, extiende `ParamField`:
   ```ts
   | { kind: 'multiline-string'; key: string; label: string; default: string; placeholder?: string }
   ```
2. Mismo fichero, añade una rama de render — espera, en realidad NO añades un renderer aquí. El catálogo sólo aporta la definición de tipo.
3. En `src/lib/components/nodes/BlueprintNode.svelte`, añade una rama `{:else if d.kind === … }` / `kind: 'number'`-style. Pero para un `ParamField` de un KIND nuevo que apunta a todos los nodos, el sitio correcto es el renderer DENTRO de `{#if def_has_a_param_field}` en lugar de los bloques `d.kind === …`. Hoy el editor de params está hardcodeado en `BlueprintNode.svelte` por `d.kind`. Si tu nuevo `ParamField.kind` está atado a un kind ESPECÍFICO de nodo (p. ej. `multiline-string` sólo en `literal.json`), añade ahí una rama nueva.
4. En `src/lib/components/Inspector.svelte`, añade un `{:else if field.kind === 'multiline-string'}` que renderice un `<textarea>`.

## Añadir un nuevo port type

Los port types viven en un sitio TS y un sitio Rust. Se inlinan como strings en disco; no hay check a compile-time sobre los colores.

### Ficheros a tocar

- `src/lib/types/blueprint.ts`: extiende la unión `PortDataType` y `PORT_COLORS` / `PORT_LABELS`.
- `src-tauri/src/blueprint.rs`: extiende el doc-comment listando los valores válidos de `Port::data_type` (hoy no usamos enum porque los strings de port type fluyen libremente por los exports).
- Opcional: `src/lib/components/nodes/BlueprintNode.svelte` — extiende las reglas `:global(.svelte-flow__handle.port-<type>)` para que el handle tenga una forma única. Hoy cada tipo tiene forma (number→círculo, string→cuadrado, boolean→diamante, array→triángulo, any→círculo hueco). Añade una para tu tipo nuevo.

Convención de nombrado: lowercase, sin guiones. `object`, `datetime`, `color` serían candidatos naturales como siguientes pasos.

## Añadir un nuevo Tauri command

### Ficheros a tocar

1. `src-tauri/src/lib.rs` — añade la función:
   ```rust
   #[tauri::command]
   fn my_command(arg: String) -> Result<MyResult, MyError> { … }
   ```
   Y añádela a `tauri::generate_handler![save_blueprint, load_blueprint, run_blueprint, my_command]`.

2. `src-tauri/capabilities/default.json` — Tauri 2 es deny-by-default para comandos custom. O bien añade el comando bajo `permissions: […]` o añádelo al ACL de la app. El camino más simple es:
   ```json
   { "identifier": "my_command", "description": "…", "commands": ["my_command"] }
   ```
   Hmm, eso depende de la sintaxis específica de ACL en Tauri 2. Testea con `tauri dev` y lee el error si el comando se rechaza.

3. `src/lib/utils/` (o un nuevo fichero como `src/utils/my-command.ts`) implementa:
   ```ts
   export async function myCommand(arg: string): Promise<MyResult> {
     return invoke<MyResult>('my_command', { arg });
   }
   ```
   Las keys de los argumentos DEBEN encajar con los nombres de parámetro Rust (case-insensitive en el wire vía el normalizer de Tauri 2).

4. (Opcional) `pnpm check` y `cd src-tauri && cargo build` para confirmar que ambos lados siguen compilando.

### Tipos de error que cruzan el IPC

Los errores que cruzan el IPC DEBEN implementar `Serialize` (a menudo aplanando a string). Véase `BlueprintError` y `ExecError` en `lib.rs` / `exec/mod.rs` para el patrón.

## Añadir un source/destination de fichero al catálogo

Aún no tenemos nodos de ficheros (`file.read`, `file.write`). Cuando los añadamos:

1. **NO** extiendas `EvalCtx` aún — `Params` basta para los paths.
2. **SÍ** añade un check en el Runner para Err… no, mejor: el ejecutor debe fallar ante ficheros ausentes en tiempo de ejecución:
   ```rust
   let content = std::fs::read_to_string(params.get("path").and_then(|x| x.as_str()).unwrap_or(""))
       .map_err(|e| ExecError::Node { node: "file.read".into(), message: format!("{e}") })?;
   ```

3. Considera introducir un modelo de permisos cuando el file picker entre en la UX (hoy sólo `io.input` acepta un valor en tiempo de run). Probablemente un futuro `RuntimeCtx` aparte de `EvalCtx`, con campos capability-scoped.

## Bumpear la versión del schema

Si cambias `Blueprint` de un modo que rompe ficheros antiguos (renombrar un campo, cambiar el `data_type` de un puerto, etc.):

1. Bumpea `pub const SCHEMA_VERSION` en `src-tauri/src/blueprint.rs`.
2. Bumpea `export const SCHEMA_VERSION` en `src/lib/types/blueprint.ts`.
3. Añade una rama de migración en `lib.rs::save_blueprint` (downgrade en write si la versión cargada es más antigua que lo que el fichero contiene tras las ediciones).
4. Añade una rama de migración en `lib.rs::load_blueprint` (migra al subir para que un fichero cargado con una versión anterior aún deserialice en la nueva struct).
5. Documenta la migración en este fichero como nueva sección bajo "Bumping the schema".

Para un cambio aditivo (no rompedor) — p. ej. un nuevo campo opcional con `#[serde(default)]` — no hace falta bump; los ficheros antiguos siguen cargando porque el campo se rellena por defecto del lado Rust.

## Lanzar la suite de tests

```bash
pnpm install
cd src-tauri && cargo test --lib              # 20 tests unitarios + 2 de integración + 1 cycle
pnpm check                                       # svelte-check / tsc
```

Si añades código Rust: al menos un test unitario positivo por cada nuevo node executor. Si añades lógica de frontend: al menos un smoke test manual de `pnpm tauri dev` del nuevo flujo.

## Añadir un nodo async (hoy, solo nodos IA)

Un nodo que debe esperar I/O (una llamada de red) no cabe en
`NodeExec`: un puntero a función no puede devolver `impl Future`. Va al
registro async.

1. **Executor**: crea `executor/nodes/<kind_snake>.rs` con

   ```rust
   pub fn exec<'a>(
       inputs: &'a Map<String, Value>,   // owned: los clona el runner
       params: &'a Map<String, Value>,
       env: &'a AiEnv,
   ) -> Pin<Box<dyn Future<Output = Result<NodeOutputs, ExecError>> + Send + 'a>> {
       Box::pin(async move { /* … */ })
   }
   ```

   La credencial NO se lee de `params`: se pide al gateway
   (`env.gateway.complete(&request)`), que la resuelve desde
   `SecretStore`.

2. **Registro**: `pub mod <kind_snake>;` y una línea en
   `setup_async_registry()` — **no** en `setup_registry()`. Un kind debe
   estar en exactamente una de las dos tablas.

3. **Catálogo**: entrada normal en `NODE_CATALOG`. Si el nodo necesita
   texto largo, usa `ParamField` tipo `textarea`.

4. **Tests**: inyecta `ai::mock::MockGateway` y llama a
   `run_graph_with(&bp, &inputs, gateway)` dentro de
   `tauri::async_runtime::block_on(...)`. Nunca uses la red ni claves
   reales en `cargo test`.

## Escribir en el log del run desde un nodo

Todo nodo puede dejar rastro en el panel **Logs** sin declarar puertos
nuevos. `EvalCtx` trae cuatro atajos y el runner rellena `node_id` y
`kind` por ti:

```rust
use serde_json::json;

pub fn exec(
    inputs: &NodeInputs<'_>,
    params: &Map<String, Value>,
    ctx: &mut EvalCtx<'_>,
) -> Result<NodeOutputs, ExecError> {
    let rows = /* … */;
    ctx.debug("candidate", json!({ "count": rows.len() }));   // oculto salvo filtro
    ctx.info("rows kept", json!(rows.len()));
    if skipped > 0 {
        ctx.warn("skipped malformed rows", json!(skipped));
    }
    Ok(single("out", /* … */))
}
```

Niveles: `debug` → `info` → `warn` → `error`. El valor es cualquier
`serde_json::Value`; los escalares (`&str`, `bool`, números) entran
por `Into<Value>`, y un `&Value` se pasa con `.clone()`.

Reglas:

- **No dupliques `node_id`/`kind` en el label**: ya están en la entrada y
  el panel filtra por ellos.
- **`io.logger` es la versión sin código** (puerto `in` + params `label`
  y `level`); úsala cuando el valor ya fluye por el grafo.
- **Si el nodo aborta con `Err(...)`, no hace falta loggear el error**: el
  runner lo adjunta al fallo junto con las entradas previas, así que el
  panel y el botón *Copy* muestran hasta dónde llegó el run.
- Los logs son efímeros: viven en el `RunResult` de esa ejecución, no se
  escriben en disco ni se serializan con el blueprint.

## Pitfalls comunes

- **Añadir un nodo pero olvidar `setup_registry()`** — su kind aparece en el palette pero cada Run devuelve `ExecError::UnknownKind`.
- **Añadir un nodo pero olvidar la entrada del catálogo correspondiente** — el canvas no puede renderizar el nodo (el drop funciona con el drag del usuario, pero `BlueprintNode.svelte` cae al texto del kind como fallback).
- **Tocar `PORT_COLORS` sin tocar `PortDataType`** — TS no se quejará a compile time (es un `Record<PortDataType, string>`), pero el palette en runtime mostrará `undefined` para cualquier nodo que tenga un puerto del nuevo tipo.
- **Poner `overflow: hidden` en `.node` dentro de `BlueprintNode.svelte`** — la saga multi-iteración sobre artefactos visuales nos enseñó: cualquier propiedad que clip-e el nodo también clip-e los handles de Svelte Flow.
- **Quitar el `:global(.canvas-host .svelte-flow__node) { background: transparent !important; … }`** — igual. Los tests no cogen la regresión visual; un `pnpm tauri dev` manual muestra cómo vuelve el bug del phantom-outline.
- **Cruzar el IPC con una struct que no `Serialize`** — Vite no avisará; el lado Rust entra en pánico con "the trait Serialize is not implemented". Tauri muestra un error genérico en devtools; el bug está del lado Rust.
- **Imports en `#[cfg(test)] mod tests` que referencian un tipo del padre sin `super::`** — los `use` al principio del fichero NO están dentro del scope del submodule de tests. Si el test usa un tipo ya importado por el código de producción, añade `use super::ExecError;` (o el equivalente) al test module, o el compilador dirá "use of undeclared type" al hacer `cargo test --lib` aunque `cargo check` pase. La regla es: el patrón `use` dentro del file NO propaga al submodule de tests. El tipado del test debe ser autosuficiente.
