# Decisiones técnicas y de seguridad

Registro de decisiones técnicas, de arquitectura y de seguridad. Cada
entrada tiene un ID estable y no se renumera. Las decisiones funcionales
(comportamiento visible para el usuario) viven en
[`../SPEC/decisions.md`](../SPEC/decisions.md).

---

## ADR-001 — `rig-core` como cliente de IA, aislado tras `AiGateway`

**Contexto.** El nodo `ai.completion` necesita hablar con proveedores
externos. Rig (`rig-core` 0.46.0) ofrece clientes tipados para OpenAI,
Anthropic, Cohere, Gemini, Groq, OpenRouter, Ollama, etc. Es una crate
pre-1.0: entre 0.42 y 0.46 `CompletionRequest` sustituyó al antiguo
builder `.prompt().send()`, y el crate pasó a llamarse `rig-core` con
`rig` como re-export.

**Decisión.** Usar `rig-core` —no el crate `rig`, que arrastra agents y
vector stores que no necesitamos— fijado con `=0.46.0`, y confinarlo en
`src-tauri/src/ai/providers.rs`. El resto del backend depende solo del
trait `AiGateway`.

**Consecuencias.** Actualizar Rig es tocar un único fichero; a cambio
perdemos las capas de agentes/RAG. `rig-core` es solo asíncrono, lo que
arrastra la decisión ADR-002.

---

## ADR-002 — Registro async separado, no un `NodeExec` asíncrono

**Contexto.** `NodeExec` es un puntero a función
(`fn(&NodeInputs, &Map, &mut EvalCtx) -> Result<NodeOutputs, ExecError>`),
elegido a propósito por ser más barato que una vtable y por convertir el
registro en un literal de datos. Un puntero a función no puede devolver
`impl Future`.

**Decisión.** Mantener `NodeExec` intacto y añadir un segundo registro,
`setup_async_registry()`, con
`AsyncNodeExec = fn(&Map, &Map, &AiEnv) -> Pin<Box<dyn Future + Send>>`.
`run_graph`/`execute` pasan a `async`; el comando `run_blueprint` también.
Los nodos síncronos no cambian una línea: el runner consulta el registro
async primero y, si no hay entrada, toma el camino síncrono de siempre.

**Consecuencias.** Los nodos async reciben *inputs owned* (el runner los
clona y suelta el mapa prestado antes del `await`); solo `ai.*` paga ese
clon. El orden topológico y el determinismo no cambian: los nodos se
esperan uno a uno en el orden de Kahn. Las alternativas descartadas
fueron boxear un futuro por nodo en el camino caliente de los 37 nodos
existentes, y bloquear con `block_on` dentro de un comando síncrono
(riesgo de pánico/deadlock por anidar runtimes).

---

## ADR-003 — Las claves API nunca viven en el blueprint

**Contexto.** La tarea original (AI-FRONT-005) pedía introducir la clave
en el panel de propiedades del nodo. En esta arquitectura todo lo que hay
en `BlueprintNode.params` se serializa en el `.blueprint.json`, que el
usuario guarda en disco, diffea y comparte.

**Decisión.** La clave no es un `param` ni un `Port`. Se configura una
sola vez en el diálogo **AI Keys** de la toolbar, y el nodo solo declara
`provider` / `model` / `base_url`. El runner resuelve la credencial en
Rust (`RigGateway` → `SecretStore`) y la inyecta en el gateway.

**Consecuencias.** Un blueprint con nodos de IA sigue siendo seguro de
compartir. El backend nunca devuelve la clave al frontend: `ai_key_status`
responde `{ provider, configured, hint }`, con un hint de los últimos 4
caracteres y solo para claves de 8+ caracteres.

---

## ADR-004 — Archivo de secretos con permisos 0600 en `app_data_dir`

**Contexto.** Opciones evaluadas: keychain del SO, fichero propio en el
directorio de datos de la app, o solo variables de entorno.

**Decisión.** Un `secrets.json` en `app_data_dir`, escrito de forma
atómica (tmp + rename) y con modo `0600` en Unix; en Windows hereda la
ACL por usuario del directorio. Todo el acceso pasa por `SecretStore`, de
modo que migrar a keychain del SO (Keychain / Credential Manager /
Secret Service) sea reimplementar un único struct.

**Consecuencias.** No hay cifrado en reposo más allá de los permisos del
sistema de ficheros: un proceso que ya corre como el mismo usuario puede
leerlo. Se aceptó a cambio de no depender de un daemon de Secret Service
(en Linux) ni de un segundo factor de desbloqueo. SI se sube el listón,
ADR-004 se sustituye, no se parchea: `SecretStore` es la única superficie.
