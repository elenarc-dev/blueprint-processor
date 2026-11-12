# Backend (Rust)

Todo el código backend vive en `src-tauri/src/`. Las dependencias Cargo están en `src-tauri/Cargo.toml`; la metadata de bundle en `src-tauri/tauri.conf.json`; los capability grants (qué `#[tauri::command]` puede invocar el frontend) en `src-tauri/capabilities/default.json`.

## Mapa de ficheros

| Fichero | LoC | Por qué existe |
|---------|-----|----------------|
| `main.rs` | 6 | Tan sólo llama a `lib::run()`. Se mantiene fino para que los entry points móviles puedan sustituirlo. |
| `lib.rs` | 150 | Handlers de comandos Tauri (`save_blueprint`, `load_blueprint`, `run_blueprint`) + writer atómico + tipos de error que cruzan el IPC. |
| `blueprint.rs` | 112 | Forma JSON del Blueprint en disco. La contraparte TS canónica está en `src/lib/types/blueprint.ts`. |
| `executor/mod.rs` | 151 | Tipos core: `Value`, `EvalCtx`, `LogEntry`, `ExecError`, `NodeExec`, `RunResult`, `RunStats`. Helpers: `null`, `value_type_name`, `single`, `empty_outputs`, `value_to_string`, `f64_to_value`. |
| `executor/broadcast.rs` | 121 | `broadcast_numeric` + `broadcast_pairs`: dispatch elementwise sobre scalar / array para ops binarias. |
| `executor/runner.rs` | 345 | `run_graph()` = Kahn topo + ejecución por nodo. Dos tests `#[cfg(test)]` de integración al final del fichero. |
| `executor/nodes/mod.rs` | 107 | El registro (`setup_registry`); declara todos los `pub mod` por nodo y el `test_helpers` compartido bajo `#[cfg(test)]`. |
| `executor/nodes/*.rs` | 20×40–112 | UN FICHERO POR KIND. Cada uno exporta `pub fn exec(...)` y contiene su propio `#[cfg(test)] mod tests`. |
| `ai/mod.rs` | ~180 | Capa de IA: `AiProviderKind`, `AiRequest`, `AiError`, `AiGateway` (trait dyn-safe), `AiEnv`, `RigGateway`, `UnconfiguredGateway`. |
| `ai/providers.rs` | ~220 | ÚNICO fichero que conoce `rig-core`. Traduce a OpenAI Chat Completions (con `base_url`) y Anthropic Messages; reintentos acotados. |
| `ai/secrets.rs` | ~230 | `SecretStore`: claves API en `<app_data_dir>/secrets.json`, escritura atómica y `0600` en Unix. |
| `ai/mock.rs` | ~45 | `MockGateway` bajo `#[cfg(test)]`: tests offline y deterministas. |

## `main.rs`

```rust
fn main() {
    blueprint_processor_lib::run()
}
```

Todo el fichero son cuatro líneas más un `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]` para que una build release en Windows oculte la consola. En mobile esto se sobreescribe vía `#[tauri::mobile_entry_point]` sobre `lib::run`.

## `lib.rs`

### Módulos / capabilities
- `mod blueprint; mod executor;` — submódulos top-level.
- Usa `serde`, `serde_json`, `thiserror`.

### Tipos

```rust
#[derive(Debug, Error)]
pub enum BlueprintError {
    Io(#[from] std::io::Error),
    Json(#[from] serde_json::Error),
    UnsupportedVersion { found: u32, expected: u32 },
    EmptyPath,
}

#[derive(Serialize)] pub struct SaveResult { pub path: String, pub bytes: usize }
```

La impl de `Serialize` para `BlueprintError` (y la equivalente para `ExecError` en `executor/mod.rs`) aplana el error a un string vía `serializer.serialize_str(&self.to_string())`. **Por qué:** el IPC de Tauri transporta errores como JSON plano sin un tipo serializable custom del lado TS, así el frontend puede decodificarlos como `unknown` y surfacearlos vía `String(err)`. El coste es perder datos estructurados; el beneficio es zero fricción con `invoke<T>`.

### Comandos

| Función | Argumento | Resultado | Capability en Tauri |
|---------|-----------|-----------|---------------------|
| `save_blueprint(json, path)` | El JSON del `Blueprint` completo + un path absoluto que el usuario eligió en un diálogo Save | `SaveResult { path, bytes }` | `capabilities/default.json` → `permissions: ["core:default", "dialog:default"]` + el comando entra al set deny-by-default de Tauri 2. |
| `load_blueprint(path)` | Path absoluto de un diálogo Open | El string JSON normalizado | Mismo set. |
| `run_blueprint(json, inputs)` | JSON del Blueprint + `HashMap<String, Value>` keyed por `params.name` de los nodos `io.input` | `RunResult` | Mismo set. Es `async` porque `ai.*` espera llamadas de red. |
| `ai_key_status()` | ninguno | `Vec<SecretStatus>` (`provider`, `configured`, `hint`) | Mismo set. Nunca devuelve la clave. |
| `ai_set_key(provider, key)` | Proveedor canónico + clave en claro | `Vec<SecretStatus>` | Mismo set. |
| `ai_clear_key(provider)` | Proveedor canónico | `Vec<SecretStatus>` | Mismo set. |

`save_blueprint` también normaliza ficheros legacy con `schema_version == 0` a la versión actual mientras pasan (véase *Bumping the schema* en [`ARCHITECTURE/recipes.md`](ARCHITECTURE/recipes.md)).

### `write_atomically(path, bytes)`

Escribe los `bytes` primero a `.filename.<pid>.<nanos>.tmp` junto al destino y luego `fs::rename`s encima. Un `.tmp` parcialmente escrito dejado por un crash no reemplaza el fichero bueno anterior; en el siguiente save el helper simplemente crea un nombre tmp nuevo. El sufijo pid+nanos evita carreras cuando varios saves ocurren en el mismo milisegundo.

### `pub fn run()`

```rust
tauri::Builder::default()
    .plugin(tauri_plugin_opener::init())
    .plugin(tauri_plugin_dialog::init())
    .invoke_handler(tauri::generate_handler![
        save_blueprint, load_blueprint, run_blueprint
    ])
    .run(tauri::generate_context!())
    .expect("error while running tauri application")
```

Plugins que usamos hoy:
- `tauri-plugin-dialog` — Diálogos Save / Open / ask desde `Toolbar.svelte`.
- `tauri-plugin-opener` — abrir links externos desde el texto de ayuda (hoy no usado en la UI global pero está habilitado para que links futuros de ayuda funcionen sin rebuilds).

`generate_context!` lee `tauri.conf.json` para los settings de ventana, identifier, metadata de bundle, y el ACL de `capabilities/default.json`.

## `blueprint.rs`

### `pub const SCHEMA_VERSION: u32 = 1;`

La única versión en producción actualmente. Increméntala en código cuando un cambio en la forma en disco sea rompedor; bump también la constante `SCHEMA_VERSION` en `src/lib/types/blueprint.ts` y escribe una migración one-shot en `lib.rs::save_blueprint` / `load_blueprint`.

### `Blueprint`

```rust
pub struct Blueprint {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub description: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub nodes: Vec<BlueprintNode>,
    pub connections: Vec<BlueprintConnection>,
    pub metadata: BlueprintMetadata,
}
```

Todos los campos excepto `schema_version`, `id`, `name`, `nodes`, `connections` llevan `#[serde(default)]` para que ficheros antiguos que no los tengan aún deserialicen (`description = ""`, `metadata = BlueprintMetadata::default()`).

### `BlueprintNode`

```rust
pub struct BlueprintNode {
    pub id: String,
    pub kind: String,                 // p. ej. "math.add"
    pub category: Option<String>,     // espeja catalog.category en TS
    pub position: NodePosition,
    pub label: Option<String>,        // sobreescribe def.label para display
    pub params: serde_json::Value,    // la forma depende del kind
    pub inputs:  Vec<Port>,
    pub outputs: Vec<Port>,
}
```

`params` llega como JSON en lugar de `Map<String, Value>` para que la forma en disco sea permisiva (ficheros antiguos con claves extra sobreviven hasta el ejecutor), y `runner.rs::execute` llama a `.as_object().cloned().unwrap_or_default()` para coercionar en el borde.

### `Port`

```rust
pub struct Port {
    pub id: String,           // machine-readable; p. ej. "in"
    pub name: String,         // etiqueta UI; p. ej. "In"
    #[serde(rename = "type")] // la clave en disco es "type", el campo Rust data_type
    pub data_type: String,    // "number" | "string" | "boolean" | "array" | "any"
}
```

La clave en disco se renombra a `type` porque `type` es una palabra reservada en Rust. Nota: el lado TS usa `type` directamente (`Port.type: PortDataType`) así que la forma en el wire ya es `type`.

### `BlueprintConnection`

```rust
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlueprintConnection {
    pub id: String,
    pub source_node: String,
    pub source_port: String,
    pub target_node: String,
    pub target_port: String,
}
```

`rename_all = "camelCase"` convierte `source_node` → `sourceNode` en el wire, así encaja uno a uno con la interface TS.

## `executor/mod.rs`

Tipos que cruzan capas y helpers puros. Sin I/O.

### Tipos

```rust
pub struct EvalCtx<'a> {
    pub node_id: &'a str,
    pub kind: &'a str,
    pub external_inputs: &'a HashMap<String, Value>,
    pub outputs: &'a mut HashMap<String, Value>,
    pub logs:    &'a mut Vec<LogEntry>,
}
```

`EvalCtx` se construye una vez por llamada a `runner.rs::execute` y se pasa por `&mut` a cada ejecutor del orden topológico. Los executors nunca ven el `EvalCtx` de otro nodo.

```rust
#[derive(Debug, Clone, Serialize)]
pub struct LogEntry { pub node_id: String, pub label: String, pub value: Value }
```

`Serialize` deja que la entrada cruce el IPC sin transformaciones. Los executors no construyen `LogEntry` directamente — `io.logger::exec` es el único llamador.

```rust
pub type NodeInputs<'a>  = HashMap<String, &'a Value>;
pub type NodeOutputs     = HashMap<String, Value>;
pub type NodeExec        =
    fn(&NodeInputs<'_>, &Map<String, Value>, &mut EvalCtx<'_>)
      -> Result<NodeOutputs, ExecError>;
```

Tres razones por las que `NodeExec` es un puntero a función (no un trait): dispatch más barato, más fácil de testear (cada nodo es una sola función), y el registro en `nodes/mod.rs` se vuelve un literal plano. El coste es que no hay composición estilo `dyn`; no la necesitamos.

### `ExecError`

```rust
#[derive(Debug, thiserror::Error)]
pub enum ExecError {
    Cycle(String),
    UnknownKind(String),
    MissingInput { node: String, port: String },
    TypeMismatch { node: String, expected: String, actual: String },
    Node { node: String, message: String },
    MissingInputValue { name: String },
}
```

El `impl Serialize` de `ExecError` aplana a un string — el mismo trade-off que `BlueprintError`. Usa `TypeMismatch` cuando un nodo lee un input vía `as_f64()` / `as_str()` etc. y recibe `None`. Usa `Node { node, message }` para todo lo demás (p. ej. división por cero, fallos de parseo de regex).

### Helpers

- `pub fn null() -> &'static Value` — Devuelve un `&'static Value::Null`. Evita la trampa de lifetime del `unwrap_or(&Value::Null)`. Hoy sin uso fuera de los tests; se mantiene para futuros helpers que necesiten un default.
- `pub fn value_type_name(v: &Value) -> &'static str` — Nombre compacto (`"number"`, `"string"`, `"array"`, `"object"`, `"boolean"`, `"null"`). Úsalo en `TypeMismatch { actual: … }` para que el string de error diga qué se encontró.
- `pub fn single(port, value) -> NodeOutputs` — Construye un map `NodeOutputs` con una entrada. Úsalo en el caso común "este nodo emite un output".
- `pub fn empty_outputs() -> NodeOutputs` — Úsalo para nodos que son sólo side-effects (`io.output`, `io.logger`): aún necesitan devolver *algo* porque `NodeOutputs` es `HashMap`, pero debería estar vacío.
- `pub fn value_to_string(v: &Value) -> String` — Coerción débil a string (`null → ""`, `bool/number → to_string`, `string → clone`, en otros casos → representación JSON). Mueve el comportamiento de `string.concat` que permite "concatenar un número".
- `pub fn f64_to_value(x: f64) -> Value` — JSON-bound: `NaN` / `±∞` → `Value::Null` porque `serde_json::Number` no puede representarlos.

## `executor/broadcast.rs`

Dos helpers enumeran la unión: scalar × scalar, scalar × array, array de igual longitud × array de igual longitud. Difieren en su tipado:

- `broadcast_numeric<F: Fn(f64, f64) -> Result<f64, ExecError>>`: ops numéricas. Devuelve un `f64` por elemento. Lo usan los `math.*`.
- `broadcast_pairs<F: Fn(&Value, &Value) -> Result<Value, ExecError>>`: ops que preservan `Value`. Lo usan `string.concat` y cualquier op cuyo tipo de salida es el mismo que el de entrada.

Ambos helpers levantan `ExecError::TypeMismatch { node, expected, actual }` cuando un lado no puede coercionar a la forma esperada — la closure nunca ve valores inválidos. Por eso todo nodo que envuelve uno de estos helpers no tiene que re-checkear tipos de input.

¿Por qué un helper y no broadcast inline? Las reglas son sutiles (el check de igual longitud, la rama asimétrica para "array a la izquierda", el match `Value::Array(_)`-vs-not para `broadcast_pairs`) y una única fuente de verdad evita drift entre ops de math y de string. Añadir un nuevo operador = añadir un fichero de nodo, registrarlo, y llamar al helper apropiado.

## `executor/runner.rs`

Tres puntos de entrada top-level (`build_incoming`, `topo_sort`, `execute`) partidos por concern para que cada uno sea testeable unitariamente. También hay dos tests de integración al final del fichero — `full_run_graph_literal_add_output` y `run_graph_detects_cycle` — que ejercitan el pipeline end-to-end con `Blueprint`s construidos a mano.

### `pub fn run_graph(bp, external_inputs) -> Result<RunResult, ExecError>`

```rust
let started = Instant::now();
let registry = nodes::setup_registry();
let incoming = build_incoming(&bp.connections);
let order = topo_sort(&bp.nodes, &bp.connections)?;
execute(&order, &bp.nodes, &incoming, &registry, external_inputs, started)
```

El `RunStats.duration_ms` devuelto es wall-clock desde la entrada a la función hasta su salida. El `Instant` `started` se enhebra hasta el final para que este número mida lo que付出了 el llamante.

### `fn build_incoming(connections) -> HashMap<node, HashMap<port, (src_node, src_port)>>`

Etapa en la que cada connection se indexa para lookup O(1) durante la ejecución por nodo. Se construye una vez al principio de `run_graph` para que el loop por nodo sea un único `HashMap::get`.

### `fn topo_sort(nodes, connections) -> Result<Vec<String>, ExecError>`

Algoritmo de Kahn con determinismo:

1. Computa `in_degree` por nodo.
2. De-duplica edges que apuntan al mismo `(target_node, target_port)` para que un edge duplicado no sobre-cuente. (Svelte Flow no puede crear dos edges con el mismo target-port porque `Editor.svelte::isValidConnection` rechaza el segundo.)
3. Empuja nodos in-degree-0 al `frontier`, los ordena.
4. Pop el más pequeño, lo añade a `order`, decrementa sucesores, empuja los recién-0 al `frontier` (ordenado).
5. Si `order.len() != nodes.len()` hay un ciclo. Reporta el primer nodo in-degree-`>0` como culprit.

**Por qué ordenar en cada paso:** el orden de referencia del grafo no es estable (Svelte Flow produce nodos en el orden que se dejaron caer en el canvas), y el runner debe producir output determinista para snapshot tests. Ordenar por ID de nodo garantiza que `run_graph` siempre elige el mismo nodo primero cuando hay elección.

### `fn execute(order, bp_nodes, incoming, registry, external_inputs, started) -> Result<RunResult, …>`

El hot loop. Por nodo:

1. Busca el `BlueprintNode` por id (error `Node { node, message: "missing from blueprint" }` si el topo sort está corrupto).
2. Busca el `NodeExec` en el registro (`ExecError::UnknownKind` si ausente).
3. Construye un map `NodeInputs`: por cada puerto de input declarado, busca la connection en `incoming` (defaulteando a `Value::Null` si está desconectado) y lee el valor producido de un `HashMap<(node_id, port_id), Value>`.
4. Coerciona `params` a `Map<String, Value>` (default vacío).
5. Construye el `EvalCtx`.
6. Llama `executor(&inputs, &params, &mut ctx)?`.
7. Inserta cada valor emitido en `(node_id, port_id) -> Value`.

Tras el loop: ensambla `RunResult { outputs, logs, stats }`. El contador `executed` se incrementa antes del insert así un nodo que falló y propagó aún se cuenta (para diagnósticos). Los tests assertan `stats.nodes_executed == 4` tras un run de 4 nodos.

### Tests de integración

- `full_run_graph_literal_add_output()` — Construye un grafo de 4 nodos (literal.number 5 → math.add.a, literal.number 3 → math.add.b, sum → io.output). Asserta `outputs["result"].as_f64() == Some(8.0)` y `stats.nodes_executed == 4`.
- `run_graph_detects_cycle()` — Construye un ciclo de 2 nodos (a → b → a). Asserta que el error encaja con `ExecError::Cycle(_)` y que el runner no entra en panic con grafos sin orden válido.

Lánzalos con `cd src-tauri && cargo test --lib`.

## `executor/nodes/mod.rs`

### Convención (para cada fichero de nodo)

```rust
pub fn exec(
    inputs: &NodeInputs<'_>,
    params: &Map<String, Value>,
    ctx:    &mut EvalCtx<'_>,
) -> Result<NodeOutputs, ExecError> { … }

#[cfg(test)]
mod tests { … }
```

El puntero a función registrado en `setup_registry` tiene que ser `pub fn exec` con exactamente esa signature. Cualquier mismatch de signature = error de compilación en la línea `r.insert("kind".into(), foo_bar::exec as NodeExec);`.

### `pub fn setup_registry() -> HashMap<String, NodeExec>`

El registro. Lo construimos en cada llamada a `run_graph` (es barato — 20 inserts en un `HashMap`). Dos razones para no memoizar: (a) los tests pueden registrar ejecutors distintos vía un registro paralelo; (b) el coste es despreciable comparado con el trabajo del ejecutor.

Cuando añadas un nodo, **añade una línea aquí** en el bloque de la categoría correspondiente.

### `pub fn setup_async_registry() -> HashMap<String, AsyncNodeExec>`

Segundo registro, solo para kinds que deben esperar I/O (hoy `ai.*`).
`AsyncNodeExec` recibe **inputs owned** y devuelve un futuro boxeado, porque
un puntero a función no puede devolver `impl Future`. Un kind debe estar en
exactamente una de las dos tablas; el runner consulta la async primero y
cae a la síncrona si no hay entrada. Ver [`decisions.md`](decisions.md) ADR-002.

### `#[cfg(test)] pub mod test_helpers`

Dos helpers expuestos a los `#[cfg(test)] mod tests` de cada nodo:

1. `ctx_with<'a>(node_id, kind, external, outputs, logs) -> EvalCtx<'a>` — Construye un `EvalCtx` con buffers vacíos. Úsalo en todo test que quiera llamar a un ejecutor.
2. `empty_inputs<'a>(null) -> NodeInputs<'a>` — Construye un map `NodeInputs` con una sola entrada `__null`. Úsalo para executors que no leen ningún puerto específico (p. ej. `io.input`) — evita que el test fugue una referencia `Value::Null` de larga vida.

NUNCA referencies estos helpers desde código de producción — están bajo `#[cfg(test)]` y no compilan en builds release.

## Visión general de los executors por nodo

Cada entrada abajo es el mapeo kind → fichero más el contrato del ejecutor (una línea). Entra en `src-tauri/src/executor/nodes/<file>.rs` para ver el cuerpo.

### Nodos literales (sin inputs, emiten params como un único output)

| Kind | Fichero | Output |
|------|---------|--------|
| `literal.number` | `literal_number.rs` | `{ value: f64 }` |
| `literal.string` | `literal_string.rs` | `{ value: String }` |
| `literal.boolean` | `literal_boolean.rs` | `{ value: bool }` |
| `literal.json` | `literal_json.rs` | `{ value: Value }` (parsea params.value con `serde_json::from_str`, defaultea `null` ante error de parse) |

### Nodos matemáticos (binarios numéricos, broadcast sobre arrays de igual longitud)

| Kind | Fichero | Params | Output |
|------|---------|--------|--------|
| `math.add` | `math_add.rs` | none | `sum: number` |
| `math.subtract` | `math_subtract.rs` | none | `diff: number` |
| `math.multiply` | `math_multiply.rs` | none | `product: number` |
| `math.divide` | `math_divide.rs` | none | `quotient: number` (división por cero → `ExecError::Node { node: "math.divide", message: "division by zero" }`) |
| `math.compare` | `math_compare.rs` | `op: select ∈ {eq, neq, gt, gte, lt, lte}` | `result: boolean` |

`math.compare` inline su propio loop de broadcast en lugar de pasar por `broadcast_numeric` porque `broadcast_numeric` coerciona el output booleano de vuelta a `f64`. Rehacer el loop es más barato que pelearse con la surface del trait.

### Nodos de string

| Kind | Fichero | Params | Output |
|------|---------|--------|--------|
| `string.concat` | `string_concat.rs` | none | `out: string` (elementwise sobre arrays de igual longitud; coerción débil vía `value_to_string`) |
| `string.length` | `string_length.rs` | none | `out: number` (chars) |
| `string.split` | `string_split.rs` | `sep: string (default ",")` | `out: array<string>` |
| `string.join` | `string_join.rs` | `sep: string (default " ")` | `out: string` |

### Nodos de array

| Kind | Fichero | Params | Output |
|------|---------|--------|--------|
| `array.length` | `array_length.rs` | none | `out: number` |
| `array.filter` | `array_filter.rs` | `threshold: number (default 0)` | `out: array<number>` (mantiene elementos ≥ threshold) |
| `array.sum` | `array_sum.rs` | none | `out: number` (TypeMismatch en input no-array o miembro no-numérico) |

### Nodos IO (tocan `EvalCtx`)

| Kind | Fichero | Comportamiento |
|------|---------|----------------|
| `io.input` | `io_input.rs` | Lee `ctx.external_inputs.get(params.name)`; ausente → `ExecError::MissingInputValue`. |
| `io.output` | `io_output.rs` | Escribe `inputs["in"]` (o `null`) en `ctx.outputs[params.name]`. Emite `empty_outputs()`. |
| `io.logger` | `io_logger.rs` | Empuja `LogEntry { node_id, label: params.label, value: inputs["in"] }` a `ctx.logs`. Emite `empty_outputs()`. |

### Añadir un nodo — resumen (véase [`ARCHITECTURE/recipes.md`](ARCHITECTURE/recipes.md) para la receta completa)

1. Crea `src-tauri/src/executor/nodes/<kind_snake>.rs` con `pub fn exec(...)` + uno o dos `#[cfg(test)]` casos usando `ctx_with`.
2. Añade `pub mod <kind_snake>;` en `nodes/mod.rs` y una línea en `setup_registry`.
3. Añade una entrada NodeDef a `NODE_CATALOG` en `src/lib/nodes/catalog.ts`.
4. (Opcional) Añade edición inline en `BlueprintNode.svelte` para nuevos `ParamField` kinds — normalmente no hace falta (`number` / `string` / `boolean` / `select` cubren las necesidades de hoy).

## `ai/` — proveedores externos

El motor no habla HTTP: `runner.rs` recibe un `Arc<dyn AiGateway>` y los
nodos `ai.*` lo usan a través de `AiEnv`. `RigGateway` resuelve la clave
desde `SecretStore` y delega en `providers::complete`, que es el único
sitio que importa `rig-core`.

**Las claves no están en el blueprint.** `BlueprintNode.params` viaja al
fichero `.blueprint.json`, que el usuario comparte; una clave ahí sería
una fuga. Por eso el nodo declara `provider`/`model`/`base_url` y la
credencial vive en el `SecretStore` (ver [`decisions.md`](decisions.md),
ADR-003 y ADR-004).

Convención de claves: `secrets.json` es un mapa plano
`{ "<provider>": "<key>" }` con nombres canónicos (`openai`,
`anthropic`); `ai_key_status` deriva el hint de los últimos 4 caracteres y
solo para claves de 8+ caracteres.

## Tests

```bash
cd src-tauri
cargo test --lib               # todos los tests unitarios + integración
cargo test --lib executor::    # sólo el crate executor
```

Cada fichero de nodo tiene al menos un test positivo más un test de error (cycle en `runner.rs`). Los nuevos nodos deberían seguir esta convención.
