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
