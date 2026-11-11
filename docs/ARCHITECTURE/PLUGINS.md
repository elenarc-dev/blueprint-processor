# Plugins de nodos — propuesta

> **Estado: propuesta, no implementada.** No hay código de plugins en el
> repositorio. Este documento estudia el problema, compara las opciones y
> fija un diseño candidato con un plan por fases. Cualquier cifra marcada
> como *a medir* es una estimación, no un resultado.

Fecha: 2026-11-11.

---

## 1. Resumen

La app permite hoy añadir nodos solo tocando el código y recompilando. La
propuesta es un formato de **plugin de nodos**: una carpeta en
`<app_data_dir>/plugins/<id>/` con un `plugin.json` (manifiesto) y un
módulo **WebAssembly** que implementa una función pura `JSON → JSON`.

Recomendación: **Extism** (`extism` 1.41.0, runtime sobre Wasmtime 49,
BSD-3) en el host Rust, con **cero capacidades en v1** (sin red, sin
sistema de ficheros, sin secretos). El nodo de plugin se ejecuta por la
misma rama async que ya usa `ai.completion`, recibe los mismos inputs
resueltos (incluido el *deflection* `params[<port>]`) y devuelve un mapa
de puertos de salida.

Lo que cuesta: una dependencia grande, ~0,5–1 s extra de arranque en frío
la primera vez que se compila cada módulo (cacheado después), y un
rediseño pequeño pero real del catálogo del frontend, que hoy es un array
estático.

---

## 2. Punto de partida (verificado en el código)

Añadir un nodo hoy son tres ediciones coordinadas a mano, sin ninguna
verificación en tiempo de compilación entre lados:

| Pieza | Fichero | Qué aporta |
|---|---|---|
| Ejecutor | `src-tauri/src/executor/nodes/<kind>.rs` | `pub fn exec(&NodeInputs, &Map, &mut EvalCtx) -> Result<NodeOutputs, ExecError>` |
| Registro | `nodes/mod.rs::setup_registry()` (37 kinds) y `setup_async_registry()` (1: `ai.completion`) | `HashMap<String, NodeExec>` / `HashMap<String, AsyncNodeExec>` |
| Catálogo | `src/lib/nodes/catalog.ts::NODE_CATALOG` | puertos, params, categoría, color |

Restricciones estructurales que condicionan cualquier diseño:

1. **`NodeExec` es un puntero a función** (`executor/mod.rs`), no un trait.
   Un nodo de plugin no puede ser un `fn` estático: necesita estado (módulo
   fueh cargado, límites, ruta). La dispatch tendrá que ser una tercera
   rama, no una entrada más en ese `HashMap`.
2. **El runner despacha por `kind`** (`runner.rs:293`) y falla con
   `ExecError::UnknownKind` si no está en ninguna tabla. Un blueprint con
   un plugin ausente falla con un mensaje claro, no se rompe.
3. **Los puertos viven en el fichero** (`BlueprintNode.inputs/outputs`).
   El editor los pinta desde el nodo guardado, no desde el catálogo
   (`blueprint-io.ts::blueprintToFlow`, `BlueprintNode.svelte`). Por eso un
   blueprint con un plugin desinstalado **sigue abriéndose**: el nodo se
   ve con su etiqueta de kind y sus puertos, en ámbar por defecto. Es una
   degradación buena y gratuita.
4. **`getDef(kind)` es síncrono y global** (`catalog.ts:581`), consumido
   por `Palette`, `Inspector`, `blueprint-io` y `BlueprintNode`. Para
   plugins tiene que pasar a leer un catálogo fusionado en runtime.
5. **La ejecución ya tiene una rama async** (`AsyncNodeExec`) que clona
   los inputs a valores propiedad y recibe un `AiEnv`. Es exactamente la
   forma que necesita un host de plugins; reutilizarla evita tocar los 37
   nodos síncronos.
6. **El *exec* no ordena nada** (`then_in`/`then_out` son decorativos: el
   orden lo fija el sort topológico sobre las aristas de datos). Un plugin
   que declare puertos `exec` se comportará como los builtins, sin
   necesidad de lógica extra.
7. **El mapa de inputs que ve el ejecutor ya está resuelto**: cable si
   existe y no es `Null`, si no `params[<port_id>]`, si no `Null`. Un
   plugin recibe, por tanto, los mismos valores efectivos que un nodo
   nativo, sin reimplementar el *deflection*.

---

## 3. Requisitos

| ID | Requisito |
|---|---|
| R1 | Añadir un nodo sin recompilar la app ni tocar el repositorio. |
| R2 | Un blueprint con plugins guarda `kind` y puertos; debe abrirse aunque falte el plugin. |
| R3 | El plugin no puede ser más poderoso que la app por accidente: sin red, sin fs y sin secretos salvo concesión explícita. |
| R4 | Un plugin colgado o malicioso no puede tumbar la app ni bloquear la UI indefinidamente. |
| R5 | Sin colisiones con kinds nativos ni entre plugins. |
| R6 | El coste en el arranque y en el tamaño del binario debe ser medible y acotado. |
| R7 | Depurable: el usuario tiene que poder ver por qué su plugin falla. |
| R8 | Los plugins no deben poder añadir tipos de puerto nuevos (eso exige color y forma estáticos en el canvas). |

---

## 4. Opciones evaluadas

| Opción | Aislamiento | Lenguajes de autor | Encaje | Coste | Veredicto |
|---|---|---|---|---|---|
| **A. WASM + Extism** | Fuerte (sandbox Wasmtime, memoria y CPU acotadas, sin syscalls) | Rust, JS, Python, Go, Zig, C/C++ … (PDK) | Bueno: una llamada JSON por nodo | 1 crate grande | **Recomendada** |
| B. Wasmtime directo | Fuerte | Los que generen wasm | Habría que definir ABI y host functions a mano | Medio-alto de desarrollo | Solo si Extism estorba |
| C. Proceso sidecar (JSON-RPC por stdio) | Depende del SO y del proceso | Cualquiera | Necesita ciclo de vida, empaquetado por plataforma y binarios por arquitectura | Alto operativo | Descartada para v1 |
| D. JS embebido (rquickjs / boa) | Medio (hay que cerrar el global) | JavaScript | Encaja, pero reintroduce un runtime y una superficie enorme | Medio | Descartada |
| E. Librería nativa (`libloading`) | **Ninguno**: mismo proceso y privilegios | Rust con ABI inestable | Crudo | Bajo de código, altísimo de riesgo | Descartada |
| F. Sub-blueprints como “nodo compuesto” | N/A (no hay código) | N/A | Reutiliza el runner tal cual | Bajo | **Complementaria**, no sustituye |

### A. Extism (recomendada)

Framework de plugins wasm con host SDK en Rust. Datos actuales:

- `extism` **1.41.0** (crates.io, octubre 2026), licencia BSD-3, mantenido
  por Dylibso; runtime construido sobre **Wasmtime 49**.
- PDKs oficiales para Rust, JS, Python, Go, Zig, C/C++ , AssemblyScript y
  .NET: el autor del plugin elige lenguaje y entrega un `.wasm`.
- El manifiesto **es** la política: `allowed_hosts` (vacío = sin red),
  `allowed_paths` (sin entradas = sin fs), `memory` (`max_pages`), y
  `timeout_ms` que interrumpe la ejecución y deja el plugin inservible,
  que es justo lo que queremos ante un bucle infinito.
- Aporta la ABI (puntero/longitud a memoria lineal) y el paso de bytes,
  que es la parte aburrida y fácil de hacer mal.

Contras honestos: es una dependencia con árbol grande (Wasmtime completo),
sube el tiempo de compilación en frío y varios MB el binario. **No he
medido el delta** en este proyecto; es el primer criterio de aceptación de
la fase 1.

### B. Wasmtime sin Extism

Da control total del `Linker` y de las host functions, y evita la capa de
manifiesto. A cambio hay que definir a mano el contrato de memoria con el
invitado, gestionar `Store`/`epoch_deadline` y escribir el binding. Es más
código propio para llegar al mismo sitio. Solo tiene sentido si Extism
resulta inaceptable por licencia, tamaño o features que no se pueden
desactivar.

### C. Sidecar

Atractivo porque permitiría plugins en Python con sus dependencias (lo que
un `.wasm` no puede) y porque el runner ya es async. Pero exige: un
binario por plataforma/arquitectura, protocolo y versionado propios,
supervisión de procesos zombis, y —lo importante— **ningún aislamiento
real**: el proceso corre con los privilegios del usuario, así que un
plugin hostil lee `secrets.json` sin más. Para v1 no compensa.

### D. JS embebido

`rquickjs` (QuickJS) o `boa_engine` permiten escribir plugins en JS sin
toolchain de wasm. El problema es que un motor JS es una superficie de
ataque y de mantenimiento enorme, y sigue sin resolver el aislamiento de
fs/red: habría que envolverlo igualmente. El PDK de JS de Extism ofrece el
mismo lenguaje de autor **sin** meter un motor en el host.

### E. Librería nativa

`libloading` + `extern "C"` es lo más barato de escribir y lo peor de
todos: ABI inestable entre versiones de Rust, `unsafe` en el borde, y un
plugin defectuoso (o malicioso) es código arbitrario dentro del proceso de
la app. Un `segfault` ahí cierra la aplicación y puede corromper el
`inventory.json` a medio escribir. Descartada sin matices.

### F. Sub-blueprints

No es un sistema de plugins, pero cubre el caso “quiero reutilizar esta
combinación de nodos como uno solo” sin código ni confianza de terceros.
Es barato y encaja con `metadata.inputs`. Merece la pena como feature
aparte; no sustituye a la extensibilidad real.

---

## 5. Diseño propuesto

### 5.1 Formato en disco

```text
<app_data_dir>/plugins/
└── com.acme.csv-tools/
    ├── plugin.json
    └── csv-tools.wasm
```

`app_data_dir` es el mismo directorio que ya usan `inventory.json` y
`secrets.json`, así que el empaquetado y las rutas por plataforma no
cambian.

### 5.2 Manifiesto

```json
{
  "api": 1,
  "id": "com.acme.csv-tools",
  "name": "CSV Tools",
  "version": "0.3.0",
  "description": "Parseo y escritura de CSV",
  "wasm": "csv-tools.wasm",
  "timeout_ms": 5000,
  "memory_max_pages": 256,
  "nodes": [
    {
      "kind": "plugin.csv-tools.parse",
      "label": "Parse CSV",
      "description": "Divide un CSV en filas y campos.",
      "entry": "parse_csv",
      "inputs": [
        { "id": "in", "name": "CSV", "type": "string" },
        { "id": "delim", "name": "Delimiter", "type": "string" }
      ],
      "outputs": [{ "id": "rows", "name": "Rows", "type": "array" }],
      "params": [
        {
          "kind": "select",
          "key": "delim",
          "label": "Delimiter",
          "default": ",",
          "options": [",", ";", "\t"]
        }
      ]
    }
  ]
}
```

Reglas de validación al cargar:

- `api` debe ser `1`. Un manifiesto con `api` mayor se rechaza con un
  mensaje que diga qué versión de la app hace falta; nunca se ignora en
  silencio.
- `kind` debe empezar por `plugin.` y casar con
  `^plugin\.[a-z0-9-]+\.[a-z0-9_]+$`. El prefijo reservado evita
  colisiones con builtins (R5) y hace la procedencia obvia en el canvas.
- Colisión con un kind ya cargado (builtin u otro plugin) → se rechaza el
  plugin entero, no el nodo suelto.
- Tipos de puerto: solo los que existen hoy (`number`, `string`,
  `boolean`, `array`, `any`, `binary`, `string-or-array`, `exec`). Un
  tipo desconocido se rechaza (R8).
- `params`: los mismos `ParamField` que ya entiende el Inspector
  (`number`, `string`, `boolean`, `select`, `textarea`). Así el panel de
  propiedades funciona sin escribir código por plugin.
- `entry` debe corresponder a una función exportada por el módulo. Se
  comprueba al cargar, no al ejecutar (R7).
- Cualquier campo `capabilities` no vacío → rechazo explícito con
  “este plugin pide capacidades que esta versión no soporta”. Mejor eso
  que cargarlo a medias.

### 5.3 Protocolo host ↔ plugin

Una sola función por nodo, un sobre JSON en ambas direcciones. Mantener la
ABI en bytes (Extism resuelve puntero y longitud) evita obligar al plugin
a conocer tipos wasm.

```jsonc
// host → plugin (argumento de `entry`)
{
  "api": 1,
  "node_id": "n7",
  "kind": "plugin.csv-tools.parse",
  "params": { "delim": "," },
  "inputs": { "in": "a;b\n1;2", "delim": ";" }
}

// plugin → host (valor de retorno)
{ "outputs": { "rows": [["a", "b"], ["1", "2"]] } }
// o
{ "error": "línea 3: número de campos inconsistente" }
```

Decisiones concretas:

- **Puertos de entrada**: el host manda **todos** los declarados, ya
  resueltos (cable > `params[<port>]` > `null`). El plugin no necesita
  replicar el *deflection* ni distinguir “no cableado” de “null”.
- **Salidas estrictas**: si el plugin devuelve un puerto no declarado, o
  no devuelve uno declarado, la ejecución falla. Un `null` silencioso
  aguas abajo es mucho más caro de depurar que un error con el kind y el
  id del nodo.
- **Errores**: `{"error": ...}`, un trap wasm, un timeout o un JSON
  inválido se convierten en `ExecError::Node { node, message }`, con el
  mismo formato que ve el usuario hoy en el panel de resultados.
- **Sin `EvalCtx`**: en v1 un plugin no puede escribir en `outputs` (no
  puede actuar como `io.output`) ni en `logs`. Si más adelante hace falta,
  el sobre gana un campo `logs` y el host lo valida; exponer `EvalCtx` en
  crudo sería regalar la mitad del runner.
- **Tamaño de respuesta**: cap duro (p. ej. 32 MB) para que un plugin no
  pueda agotar la memoria del host devolviendo un array gigante.

### 5.4 Integración en el runner

El cambio estructural es pequeño y va en la línea de lo que ya existe:

1. En `executor/mod.rs`, `AiEnv` pasa a `ExecEnv`:

   ```rust
   pub struct ExecEnv {
       pub node_id: String,
       pub ai: Arc<dyn AiGateway>,
       pub plugins: Option<Arc<PluginHost>>,
   }
   ```

   `AsyncNodeExec` recibe `&ExecEnv`. Afecta a `ai_completion` y a sus
   tests; el resto de nodos síncronos no se toca.

2. En `runner.rs::execute`, tras la comprobación de las dos tablas, una
   tercera rama:

   ```rust
   if let Some(def) = plugins.lookup(&bp_node.kind) {
       let owned = owned_inputs(&inputs, &params);   // ya existe el patrón
       let outs = plugins.call(def, owned).await?;
       …
       continue;
   }
   ```

   Se coloca antes del `executor` síncrono y después de la rama async, o
   se unifica con ella: ambas necesitan inputs propiedad y `await`.

3. `PluginHost`:

   ```rust
   impl PluginHost {
       pub fn discover(dir: &Path) -> Result<PluginHost, PluginError>;
       pub fn lookup(&self, kind: &str) -> Option<&PluginNode>;
       pub async fn call(&self, node: &PluginNode, input: Map<String, Value>)
           -> Result<NodeOutputs, ExecError>;
   }
   ```

   `call` hace `spawn_blocking` (la ejecución wasm es CPU y no debe
   bloquear el runtime async de Tauri), compila el módulo una vez y lo
   cachea, y crea una instancia `Plugin` por llamada: las instancias no
   son `Sync` y compartirlas entre nodos concurrentes es pedir corrupción
   de estado.

4. **Timeouts**: `timeout_ms` del manifiesto con un techo duro del host
   (p. ej. 30 s). Un plugin que lo excede devuelve error y su instancia se
   descarta; el grafo sigue. El epoch/interrupt de Wasmtime no permite
   reanudar, que aquí es exactamente lo deseable.

5. **Tests**: un `.wasm` de juguete versionado en
   `src-tauri/tests/fixtures/` (identidad, error y bucle infinito) más un
   `PluginHost` construido sobre un directorio temporal — mismo patrón
   `TmpFile` que ya usan los nodos de fichero. Alternativa a valorar: un
   fixture `.wat` compilado en el test, que evita binarios en el repo si
   Extism acepta `wat` directamente (el árbol de Wasmtime lo incluye, pero
   hay que comprobarlo).

### 5.5 Frontend

El catálogo deja de ser un array inmutable y pasa a ser la unión de dos
fuentes:

1. `catalog.ts` sigue exportando `NODE_CATALOG` (builtins) y añade una
   categoría estática `'Plugin'` con color neutro (slate) en
   `CATEGORY_COLORS` y `CATEGORIES`.
2. Nuevo `src/lib/nodes/plugin-catalog.svelte.ts`: un `$state` con las
   definiciones devueltas por un comando `plugin_list`, más
   `allDefs()` y un `getDef` que consulta ambas listas.
3. `Palette.svelte`, `Inspector.svelte` y `blueprint-io.ts` cambian el
   import directo por el helper. Son cuatro puntos: la paleta, el bloque
   de parámetros del Inspector (que ya pinta `ParamField[]` de forma
   genérica, así que un nodo de plugin gana panel de propiedades gratis),
   la lista “Available kinds” del Inspector (enumera `NODE_CATALOG` y debe
   mostrar el catálogo fusionado) y `getDef`.
4. Los handles y el `isValidConnection` no cambian: un tipo de puerto
   existente ya se pinta y se conecta como cualquier otro.
5. **Nodo huérfano**: al abrir un blueprint cuyo plugin no está, el canvas
   ya funciona sin cambios — el nodo se pinta con su `kind` como etiqueta,
   sus puertos guardados y el color de fallback. El Inspector, en cambio,
   hoy envuelve todos sus campos en `{:else if def}`: sin definición no
   muestra nada, así que hace falta una rama explícita “plugin no
   instalado” con el `id` que falta. Al ejecutar, el error ya es claro
   (`Unknown node kind 'plugin.x.y'`). Nada de borrar nodos ni de
   reescribir el fichero.
6. Recarga: comando `plugin_reload` + botón en el diálogo de plugins. No
   requiere reiniciar la app porque no hay nada compilado en el binario.

7. **Permisos de Tauri**: los comandos propios de la app (`plugin_list`,
   `plugin_reload`) no necesitan entradas nuevas en
   `src-tauri/capabilities/default.json`, igual que `save_blueprint` o
   `run_blueprint` hoy. Ese fichero solo cambia si se añade un plugin de
   Tauri, cosa que esta propuesta no requiere.

### 5.6 Seguridad

Modelo de amenaza: el plugin es código de terceros. En v1 **no puede**
hacer nada salvo transformar los datos que recibe.

| Vector | Defensa v1 |
|---|---|
| Red | `allowed_hosts` vacío → sin HTTP. No se compila el registro HTTP del host. |
| Sistema de ficheros | Sin WASI mounts, `allowed_paths` vacío. |
| Secretos / claves IA | El host no pasa `ExecEnv.ai` al plugin; el sobre solo lleva inputs y params. |
| CPU infinita | `timeout_ms` + techo duro del host. |
| Memoria | `memory.max_pages` en el manifiesto. |
| Abuso del bridge | El plugin no registra comandos IPC ni toca `EvalCtx`; no ve `app_data_dir`. |
| Suplantación | Los kinds van bajo `plugin.` y la UI muestra el plugin de origen. |
| Datos de salida | Cap de tamaño y validación estricta de puertos. |

Lo que **no** protege: un plugin puede devolver datos incorrectos a
propósito. Eso no se arregla con sandbox; se arregla con confianza y, más
adelante, con firma y con un registro curado.

Instalación: nunca se descarga wasm de una URL. Se copia una carpeta a
`plugins/` o se instala desde un `.zip` con el diálogo del sistema, y la
primera carga de cada plugin muestra un aviso único con su `id`,
`version` y un hash SHA-256 del `.wasm` (para que una actualización
silenciosa sea visible). Firma verificable: fase 3.

### 5.7 Compatibilidad de blueprints

- `schema_version` **no** cambia: un nodo de plugin es un nodo normal en
  disco (`kind`, `inputs`, `outputs`, `params`). Nada nuevo que migrar.
- Un blueprint con plugins sigue siendo válido para quien no los tiene:
  se abre, se ve, y solo falla al ejecutar ese nodo, con un mensaje que
  nombra el plugin que falta.
- Al contrario: los blueprints existentes no ganan nada ni se reescriben.
- El kind es el contrato. Renombrar un kind en el plugin rompe los
  ficheros guardados, así que la documentación del plugin debe tratarlo
  como API pública.

---

## 6. Plan por fases

### Fase 1 — Núcleo

- Dependencia `extism` (pin exacto, sin features de HTTP/FS del host si es
  posible), `plugins/{mod,manifest,host}.rs`, tercera rama en el runner,
  `ExecEnv`, comandos `plugin_list` / `plugin_reload`, un `.wasm` de
  fixture y sus tests.
- Criterios de aceptación:
  - un blueprint con `literal.string` → nodo de plugin → `json.stringify`
    → `io.output` produce el resultado esperado en un test de integración;
  - un plugin que devuelve un puerto no declarado falla con
    `ExecError::Node` que incluye el kind;
  - un plugin en bucle infinito se corta por timeout y el run termina con
    error, sin colgar el proceso;
  - los 241 tests actuales siguen pasando;
  - **delta medido** de tamaño de binario y de tiempo de compilación en
    frío, documentado aquí antes de seguir.

### Fase 2 — Experiencia de autor

- Plantilla de plugin en Rust con el PDK + script de build, y receta en
  `ARCHITECTURE/recipes.md`.
- Diálogo de plugins en la UI: lista, origen, versión, hash, activar y
  desactivar por plugin (desactivar = no cargar, los nodos quedan
  huérfanos), errores de manifiesto legibles.
- `logs` opcionales en el sobre, mapeados a `LogEntry` con el `node_id`.
- Segundo fixture en un PDK distinto (JS) para demostrar que el formato no
  está atado a Rust.

### Fase 3 — Capacidades y distribución

- `capabilities` en el manifiesto: `fs.read`/`fs.write` con rutas
  concedidas por el usuario, `http` con allowlist, `secrets` por nombre
  (nunca el valor en claro).
- Firma de plugins y actualizaciones verificadas.
- Registro/marketplace (fuera del alcance de esta propuesta).

---

## 7. Riesgos y preguntas abiertas

| Riesgo | Impacto | Mitigación |
|---|---|---|
| Tamaño del binario y tiempo de compilación | Medio | Medir en F1; si es inaceptable, evaluar Wasmtime directo o `wasmi` |
| Deriva de la API de Extism/Wasmtime | Medio | Pin exacto, igual que `rig-core`; los plugins solo dependen del sobre JSON, no del crate del host |
| El PDK del autor y el host se desincronizan | Bajo | `api: 1` en manifiesto y sobre; rechazo explícito de versiones |
| Rendimiento por llamada (serializar JSON) | Bajo | Un nodo ya cuesta µs; medir con un grafo de 10k llamadas antes de optimizar |
| Sin depurador dentro del plugin | Medio | Errores con mensaje del plugin + `logs` en F2 |
| Confianza del usuario | Alto | Cero capacidades, aviso de instalación, hash visible |

Preguntas abiertas:

1. ¿Extism compila con `default-features = false` sin arrastrar los
   registros HTTP/FS del host? Hay que verificarlo con un build real: es
   la diferencia entre “sin red por política” y “sin red por ausencia de
   código”.
2. ¿Merece la pena permitir `params` de tipo `file`/`folder` con el
   `pathPicker` nativo en v1? Implicaría que el host abra el diálogo, no
   el plugin.
3. ¿Los plugins se cargan al arrancar siempre, o solo al abrir un
   blueprint que los usa? Lo segundo reduce el arranque pero complica el
   catálogo (la paleta tendría que esperar).
4. ¿Un plugin puede declarar varios nodos que compartan un wasm? El diseño
   lo permite (una `entry` por nodo); conviene confirmarlo como intención
   y no como accidente.

---

## 8. Qué no haría

- Cargar librerías nativas (`.so`/`.dll`/`.dylib`): sin aislamiento.
- Ejecutar JS de plugins dentro del webview: rompe la autoridad del runner
  en Rust y da acceso al DOM y al bridge IPC.
- Permitir tipos de puerto nuevos desde un manifiesto: exigen CSS y
  colores estáticos; un tipo desconocido se rechaza.
- Dar a los plugins acceso a `secrets.json`, al gateway de IA o a
  comandos IPC.
- Descargar plugins de una URL en v1.
- Añadir `capabilities` “por si acaso” antes de tener el núcleo probado.

---

## 9. Referencias

- `src-tauri/src/executor/mod.rs` — `NodeExec`, `AsyncNodeExec`, `EvalCtx`,
  helpers compartidos.
- `src-tauri/src/executor/runner.rs` — dispatch por kind y rama async.
- `src-tauri/src/executor/nodes/mod.rs` — `setup_registry`,
  `setup_async_registry`.
- `src/lib/nodes/catalog.ts` — `NODE_CATALOG`, `ParamField`, `getDef`.
- `src/lib/utils/blueprint-io.ts` — materialización de nodos y puertos.
- `docs/ARCHITECTURE/decisions.md` (ADR-002) — precedente del registro
  async para nodos que esperan I/O.
- Extism: <https://extism.org>, crate `extism` 1.41.0, `Manifest`
  (`allowed_hosts`, `allowed_paths`, `timeout_ms`, `memory`).
