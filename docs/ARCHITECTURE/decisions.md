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

## ADR-003 — Los proveedores de IA y sus claves nunca viven en el blueprint

**Contexto.** La tarea original (AI-FRONT-005) pedía introducir la clave
en el panel de propiedades del nodo. En esta arquitectura todo lo que hay
en `BlueprintNode.params` se serializa en el `.blueprint.json`, que el
usuario guarda en disco, diffea y comparte.

**Decisión.** La clave no es un `param` ni un `Port`. Se configura una
sola vez por máquina en el diálogo **AI Providers** de la toolbar, y el
nodo solo declara el modelo que quiere (ADR-005). El runner resuelve la
credencial en Rust (`RigGateway` → `SecretStore`) y la inyecta en el
gateway.

**Consecuencias.** Un blueprint con nodos de IA sigue siendo seguro de
compartir. El backend nunca devuelve la clave al frontend: los comandos
`ai_providers` / `ai_save_provider` / `ai_remove_provider` /
`ai_clear_provider_key` responden `Vec<ProviderStatus>` (`id`, `kind`,
`label`, `baseUrl`, `models`, `configured`, `hint`), con un hint de los
últimos 4 caracteres y solo para claves de 8+ caracteres. Ningún DTO que
cruce el IPC puede contener `api_key`.

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

---

## ADR-005 — Catálogo global de proveedores; el nodo solo elige modelo

**Contexto.** Cada nodo `ai.completion` declaraba `provider` / `model` /
`base_url` en sus `params`. Con dos destinos en la misma máquina (OpenAI
de pago y un Ollama o LM Studio local) el nodo acababa repitiendo wire
protocol y endpoint, y un blueprint no podía decir "usa el proveedor local
de esta máquina" sin hardcodear la URL. Además el diálogo de claves ya
existía y solo guardaba `kind → clave`, sin sitio para el endpoint ni para
la lista de modelos.

**Decisión.** El catálogo de proveedores es configuración global de la
app, no contenido del blueprint. `secrets.json` pasa a
`{ "providers": [ { id, kind, label, base_url, models, api_key } ] }` y el
nodo conserva un único param, `model`, con valor `providerId::model`. El
separador es `::` —no `:`— y el split se hace en la **primera** aparición,
porque una etiqueta de Ollama ya es `llama3.1:70b`. El diálogo pasa a ser
el CRUD completo (`AiProvidersDialog.svelte`) y expone el mismo catálogo al
Inspector vía `src/lib/utils/ai-config.svelte.ts`.

**Resolución (`ai_completion.rs::resolve_target`).** Cascada, señal más
fuerte primero: 1) `providerId::model` → ese proveedor (error explícito si
no está configurado); 2) catálogo vacío → comportamiento pre-catálogo con
los params legacy; 3) el nombre de modelo lo ofrece algún proveedor (match
exacto y luego case-insensitive); 4) el `provider` legacy elige el *kind* y
se toma el primer proveedor configurado de ese kind; 5) un único proveedor,
o el primero que tenga clave. Modelo vacío → primer modelo del proveedor, o
el `default_model()` del kind si no lista ninguno. `base_url` legacy gana al
del proveedor salvo cuando el nodo nombra el proveedor con `::`, que es una
elección deliberada de endpoint.

**Migración.** `SecretStore::read()` acepta las dos formas: un array
(actual) o el mapa plano `kind → clave` (legacy), que se normaliza a
registros cuyo `id` es el nombre del kind. Escribe siempre en forma de
array. La lectura es tolerante: una entrada ilegible se descarta con una
línea de log en vez de tumbar el catálogo entero. `schema_version` del
blueprint no cambia — los params legacy simplemente siguen siendo válidos,
y el Inspector los preserva (`...d.params` al guardar) aunque ya no los
edite.

**Consecuencias.** El `id` de un proveedor **nunca se renombra**: es lo que
guardan los blueprints, así que renombrar solo toca `label`. Un draft con
`id` vacío hace que el almacén genere uno a partir del kind (`openai`,
`openai-2`, `anthropic`, …). Proveedor
sin clave pero con `base_url` = servidor local: `RigGateway` envía la clave
de relleno `LOCAL_API_KEY` (`"not-needed"`) en lugar de fallar. El seam
`AiGateway::providers()` tiene default `Ok(Vec::new())`, así que solo
`RigGateway` (el almacén real) y `MockGateway` (catálogo sembrado para un
test) lo sobrescriben; `UnconfiguredGateway` y los gateways de tests que no
usan IA no necesitan implementarlo. Alternativas descartadas:
dejar los params por nodo duplicados en el diálogo (dos fuentes de verdad
que se contradicen) y empotrar proveedores predefinidos (OpenAI, Groq,
Ollama…) en el backend, que obligaría a una release por proveedor nuevo.
