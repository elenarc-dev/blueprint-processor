# Decisiones funcionales

Registro de decisiones de comportamiento visible para el usuario. Cada
entrada tiene un ID estable y no se renumera. Las decisiones técnicas y de
seguridad viven en
[`../ARCHITECTURE/decisions.md`](../ARCHITECTURE/decisions.md).

---

## FD-001 — `ai.completion` tiene dos entradas: `Prompt` y `Data`

**Contexto.** Un nodo que solo acepta un prompt obliga a construir el
mensaje entero con `string.concat` antes de llamar al modelo. Lo natural
en un pipeline es enviar una instrucción fija con los datos que llegan de
otro nodo.

**Decisión.**

- `Prompt` (texto) se puede cablear o escribir en el propio nodo
  (parámetro `Prompt (default)`). El cable gana, como en el resto de la
  app.
- `Data` (cualquier valor) llega de otro nodo. No tiene editor inline:
  o está cableado, o no hay datos.
- Combinación: si el prompt contiene `{{data}}`, cada aparición se
  sustituye por el valor serializado (los objetos/arrays se envían como
  JSON). Si no lo contiene, los datos se añaden tras una línea en blanco.
  Si hay placeholder pero no hay datos, el nodo **falla** en vez de enviar
  una plantilla con un hueco.
- Si el prompt está vacío, el nodo falla aunque haya datos: los datos no
  son una instrucción.
- Un array de datos significa "una llamada por elemento": escalar×array
  hace broadcast, dos arrays de igual longitud se emparejan uno a uno, y
  longitudes distintas dan error. Un array vacío produce una salida vacía
  sin llamar al modelo. Para enviar un array entero como un único
  payload, se pasa antes por `json.stringify`.

**Salida.** `Out` es texto, y es un array siempre que lo fuera alguna de
las entradas (un array de 1 elemento sigue siendo un array de 1
elemento), conservando el orden de entrada.

**Consecuencias.** Un mismo nodo sirve para "resume esto" y para "clasifica
cada fila de este dataset" sin nodos auxiliares. El coste es explícito:
N filas = N llamadas, y `concurrency` acota cuántas van en vuelo.

---

## FD-002 — Los proveedores de IA se configuran una vez por máquina; el nodo solo elige modelo

**Contexto.** Cada nodo `ai.completion` repetía proveedor, endpoint y modelo
en sus params. Con dos destinos en la misma máquina (una cuenta de pago y un
modelo local) el usuario reescribía lo mismo en cada nodo, y un blueprint no
podía decir "usa lo que esta máquina tenga configurado" sin hardcodear una
URL.

**Decisión.**

- La toolbar tiene **AI Providers**, un CRUD global de proveedores: nombre,
  tipo de wire (`OpenAI-compatible` o `Anthropic`), `Base URL`, lista de
  modelos (uno por línea) y API key. Es la única manera de introducir una
  clave o un endpoint.
- El nodo tiene un único campo de destino, *Model*, como desplegable agrupado
  por proveedor. Lo que se guarda en el blueprint es
  `providerId::model` (p. ej. `ollama::llama3.1:70b`).
- Dejar *Model* vacío significa "el proveedor y el modelo por defecto": se
  usa el único proveedor configurado (o el primero con clave) y su primer
  modelo.
- Un proveedor sin clave pero con `Base URL` es válido y aparece en el
  desplegable: es un servidor local (Ollama, LM Studio). Sin clave y sin
  `Base URL`, el diálogo pide una de las dos y el run reporta que falta la
  clave.
- El nombre visible de un proveedor se puede renombrar libremente sin afectar
  a los blueprints: lo que estos guardan es su id, que no cambia. **Eliminar**
  un proveedor sí afecta: un nodo que lo referenciaba falla con un mensaje
  que lista los proveedores configurados.
- Los blueprints guardados antes de esta decisión siguen ejecutando igual:
  si un nodo conserva params `provider` y/o `base_url`, esos definen el
  destino cuando el nodo no nombra un proveedor con `::`.
  `docs/examples/AI-blueprint.json` es un ejemplo real de esa forma.

**Consecuencias.** Dos blueprints en la misma máquina comparten proveedor,
clave y endpoint: cambiar la clave o el endpoint en el diálogo afecta a
todos. Un blueprint viaja limpio (sin credenciales ni URLs locales), pero
abrirlo en otra máquina exige configurar allí un proveedor con el mismo id o
re-elegir modelo en el Inspector. El modelo que ofrece el desplegable es la
lista que el usuario escribió en el proveedor: se puede escribir cualquier
otro modelo a mano y se envía tal cual. Decisión técnica correspondiente:
ADR-005.

## FD-003 — Un blueprint guardado se puede incrustar como un nodo

**Contexto.** Los pipelines grandes repiten subgrafos (una secuencia de
limpieza, una llamada con reintentos). Copiar esos nodos en cada grafo
multiplica el mantenimiento y rompe la lectura del canvas.

**Decisión.**

- El nodo `blueprint.subgraph` guarda únicamente el **path** del blueprint
  que incrusta. En el canvas aparece como un nodo más: una entrada `Then`
  más los inputs declarados por el blueprint incrustado, y una salida
  `Then` más una salida por cada nodo `Output` (con su `params.name`).
- El fichero incrustado es la autoridad: al elegir otro path, el nodo
  recarga sus puertos y se etiqueta con el nombre del blueprint. Los wires
  que apuntaban a puertos que ya no existen se descartan al guardar y al
  aplicar la nueva forma.
- Al ejecutar, el runner carga el fichero, le pasa los valores cableados
  como `external_inputs` (mismo contrato que un run normal) y devuelve sus
  outputs. Un blueprint incrustado puede a su vez incrustar otros, hasta 8
  niveles; un ciclo (directo o indirecto) falla con un mensaje claro.
- Si el fichero no está disponible o no es válido, el nodo falla con la
  ruta en el mensaje y el Inspector muestra el error.

**Consecuencias.** Dos grafos comparten el mismo subgrafo sin duplicarlo, y
editarlo en su propio fichero afecta a todos los que lo incrustan en el
siguiente run (no hay copia embebida). El grafo deja de ser autocontenido:
compartirlo exige compartir también los ficheros incrustados y mantener sus
rutas. Decisión técnica correspondiente: ADR-006.

## FD-004 — `ai.completion` puede llamar herramientas, en dos formas

**Contexto.** Un prompt aislado no puede consultar datos actuales ni
ejecutar un subgrafo. Se necesitan tools sin convertir el nodo en un agente
sin límites ni obligar a meter endpoints en cada blueprint.

**Decisión.**

- **App-level tools.** La toolbar tiene **AI Tools**, un CRUD global de
tools HTTP (nombre, descripción, JSON Schema, método, URL, cabeceras). El
nodo las selecciona por id desde el Inspector. La definición vive en la
máquina, nunca en el blueprint.
- **Tools de grafo.** El nodo `ai.tool` expone un blueprint guardado como
  tool: su salida `Tool` se cablea al puerto `Tools` de `ai.completion`
  (varios se agrupan con `array.create`). El modelo ve el nombre, la
  descripción y el JSON Schema; al llamarla, sus argumentos se convierten
  en inputs externos del blueprint y los outputs vuelven como JSON.
- El bucle es acotado (`Max tool rounds`, 6 por defecto, máximo 16): el
  modelo pide, el nodo ejecuta, el resultado vuelve. Al agotarse, el run
  falla con un mensaje que pide subir el límite o simplificar la tarea.
- Un error de una tool se devuelve al modelo como resultado de esa llamada
  (puede corregir los argumentos); no aborta el run. Un nombre de tool
  duplicado o una app tool inexistente sí falla antes de llamar al modelo.
- Sin tools configuradas el nodo se comporta exactamente como antes.

**Consecuencias.** Las tools de nodo son la vía para "el modelo ejecuta un
pipeline", y las app-level para "el modelo consulta un servicio". Ambas
comparten el límite de anidamiento de FD-003. Una app tool sin credenciales
propias debe llevar su `Authorization` en las cabeceras; no hay tools
nativas, OAuth ni descubrimiento de esquema. Decisión técnica
correspondiente: ADR-006.

## FD-005 — `ai.vision` admite un número ilimitado de imágenes

**Contexto.** Un prompt de texto no sirve para comparar varias imágenes ni
para analizar un lote. Limitar el nodo a una imagen obligaría a repetir la
llamada y perder la relación entre ellas; limitarlo a N ranuras fijas choca
con lotes de tamaño variable.

**Decisión.**

- El nodo `ai.vision` tiene un `Prompt` (puerto + default en el cuerpo) y
  ranuras `Image 1 … N` que se añaden con +/− (`# Image slots`).
- Cada ranura acepta **una imagen base64 o un array de imágenes**, así que
  el total por petición no está acotado por las ranuras. Un `file.read`
  multi-fichero o un `array.create` alimentan una sola ranura sin límite.
  Las ranuras se limitan a 64 por ergonomía del canvas; los proveedores
  imponen además sus propios máximos por petición, que se reportan tal cual.
- Las imágenes se aplanan en orden numérico de ranura (no de iteración) y
  se validan como base64 antes de llamar al proveedor. El MIME se detecta
  por magic bytes; el payload desconocido se declara `image/png`.
- Un prompt escalar hace una llamada con todas las imágenes; un prompt
  array hace una llamada por elemento con las mismas imágenes (broadcast),
  con `concurrency` como límite de peticiones en vuelo. El output espeja la
  forma del prompt, como en `ai.completion`.
- Sin al menos una imagen el nodo falla antes de la red. El nodo no ejecuta
  tools: es un transformador multimodal, no un agente.

**Consecuencias.** Un lote de imágenes se procesa en una sola llamada, y el
coste depende del modelo elegido. El límite práctico lo pone el proveedor:
una petición con demasiadas imágenes o demasiado grandes devuelve su error
verbatim. Decisión técnica: ADR-006 (el wire multimodal vive junto al
tool-calling en `ai/providers.rs`).

## FD-006 — Un subgrafo se edita en el mismo canvas, anidado, con vuelta al padre

**Contexto.** Abrir el blueprint incrustado como si fuera otro documento
obligaba a recargar mentalmente el grafo padre al volver y perdía los
cambios no guardados del padre. A la vez, editar el subgrafo "dentro" del
canvas sin salir de la aplicación tiene que dejar claro en qué nivel se
está trabajando.

**Decisión.**

- Desde el cuerpo de un nodo `blueprint.subgraph` / `ai.tool` (botón
  **Open embedded blueprint**) o desde el Inspector se abre el fichero
  incrustado en el mismo canvas. El grafo padre se apila con su estado
  actual, incluidos los cambios sin guardar.
- Una barra de migas bajo la toolbar muestra la cadena completa
  (`raíz › sub › sub-sub`). Cada ancestro es clicable y **← Parent graph**
  vuelve un nivel. No hay un límite propio de niveles: el runner sigue
  imponiendo su máximo de 8 para ejecutar.
- `Save` guarda el nivel activo (el hijo si se abrió un subgrafo). `Load` y
  `New` descartan la pila porque reemplazan el documento entero.
- Al volver se refrescan los puertos del nodo contenedor desde el fichero
  editado; si el subgrafo cambió de interfaz, los wires a puertos
  desaparecidos se podan como en cualquier cambio de forma.
- Abrir un path que ya está en la cadena se rechaza (evita ciclos de
  navegación y niveles duplicados). Se rechaza también mientras hay un run
  en curso.

**Consecuencias.** El padre conserva sus cambios sin guardar mientras se
edita el hijo, pero no hay historial de navegación más allá de la pila
actual: volver descarta los niveles hijos, incluidos sus cambios no
guardados (el usuario debe guardarlos antes). El historial de inputs de
cada nivel se guarda bajo su propio path o `unsaved:<id>`; cambiar de nivel
no lo migra.

Al volver, si el nivel activo tiene cambios sin guardar, se pide
confirmación antes de descartarlo. La comparación usa el documento
serializado (sin `updated_at` ni los puertos que el editor re-deriva del
fichero), de modo que abrir un subgrafo y volver sin tocar nada no
dispara el aviso. `Save` rearma la línea base.

## FD-007 — Guardar la selección como blueprint y dejarla como nodo Subgraph

**Contexto.** Un grafo grande se vuelve difícil de leer y de reutilizar. El
usuario necesita extraer un conjunto de nodos a un fichero propio sin
reconstruirlo a mano ni volver a cablearlo en el grafo original.

**Decisión.** El Toolbar ofrece **Group as…** cuando hay al menos un nodo
seleccionado. La acción:

- compone un blueprint nuevo con los nodos seleccionados y solo los wires
  cuyo origen y destino están dentro de la selección (las posiciones se
  conservan);
- pide ruta con el diálogo nativo y escribe el fichero con `save_blueprint`;
- sustituye en el grafo actual los nodos seleccionados por un único nodo
  `blueprint.subgraph` cuyo `params.path` apunta al fichero recién creado,
  colocado en el centroide de la selección. El nodo recibe los puertos que
  el blueprint compuesto declara (`portsForBlueprint`), los mismos que el
  sync de la página derivaría del fichero.

**Consecuencias.** Los cables de datos que entraban desde fuera se convierten
en entradas del grupo: cada cable declara su propia entrada con un `id`
derivado del nodo emisor y del puerto (`read_file_path`) y con sufijo
numérico si se repite, así que dos consumidores del mismo nombre de puerto no
se pisan ni compiten por una sola entrada. El `name` visible añade el nodo de
origen solo cuando repetiría etiqueta. Se inyecta un nodo `blueprint.input`
que alimenta al nodo interno que antes recibía el valor. En el grafo padre esos
cables se reconectan al nodo Subgraph y en ejecución el executor del
subgrafo los entrega al hijo como `external_inputs` con ese id, así que el
valor sigue el mismo camino que antes del agrupado. Los cables de salida se
reconectan contra `then_out` y los nombres de los `io.output` internos, y las
salidas siguen ramificándose. Lo que no tiene contrapartida en el grupo se
descarta. La barra de estado informa de cuántos se reconectaron y cuántos se
perdieron. Los
`blueprint.input` que ya existían en la selección no se copian: el nodo
inyectado es el único que se traslada, porque `metadata.inputs` pertenece al
blueprint nuevo. El Toolbar ofrece **Undo group** mientras no se cargue o
cree otro documento: restaura el nivel tal como estaba antes del intercambio
(nodos, cables y selección) en un único paso, pero no borra el fichero
creado ni deshace ediciones posteriores a otros nodos.
