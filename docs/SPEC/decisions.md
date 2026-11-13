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
