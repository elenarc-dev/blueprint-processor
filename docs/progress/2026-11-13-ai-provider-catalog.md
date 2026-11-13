---
type: progress
project: blueprint-processor
created: 2026-11-13
updated: 2026-11-13
status: completed
tags: [progress]
---

# Catálogo global de proveedores de IA

## Objective

Mover la configuración de proveedores y modelos de IA desde los params de
cada nodo `ai.completion` a una configuración global, reutilizando la ventana
que ya existía para las API keys (Toolbar → **AI Keys** → **AI Providers**).
El nodo pasa a tener un único campo, *Model*, donde se elige proveedor +
modelo del catálogo configurado.

Restricciones que guiaron el trabajo:

- Reaprovechar el diálogo existente en lugar de crear una ventana nueva.
- La API key jamás viaja en el blueprint ni vuelve al frontend (ADR-003).
- Los blueprints y los `secrets.json` ya guardados siguen funcionando sin
  migración manual; `schema_version` no cambia.
- Todos los tests siguen siendo offline (`MockGateway`).

## Work Completed

### Backend (`src-tauri/`)

- `ai/secrets.rs` reescrito como catálogo global. `secrets.json` ahora es
  `{ "providers": [ { id, kind, label, base_url, models, api_key } ] }`.
  Lectura tolerante vía `serde_json::Value` + `normalize()`: una entrada
  ilegible se descarta con una línea de log en lugar de tumbar el catálogo.
  El mapa plano legacy `{ "<kind>": "<clave>" }` se normaliza a registros
  cuyo `id` es el nombre del kind. Escritura siempre en array, tmp +
  `rename`, permisos `0600`.
- `ProviderRecord` / `ProviderDraft` / `ProviderStatus`
  (`#[serde(rename_all = "camelCase")]`, ninguno con `api_key`).
  `SecretStatus` eliminado. API del almacén: `records()`, `statuses()`,
  `provider_infos()`, `record(id)`, `save()`, `remove()`, `clear_key()`.
- `id` estable y canónico (`canonical_id`: alfanumérico ASCII + `.` `-` `_`,
  minúsculas; búsqueda case-insensitive con `lookup_id`). Un draft sin `id`
  recibe uno generado a partir del kind (`openai`, `openai-2`, …). Un id
  escrito a mano (`ollama`) es válido.
- `ai/mod.rs`: `ProviderInfo` (sin clave: `has_key` la sustituye),
  `AiGateway::providers()` con default `Ok(Vec::new())`,
  `AiRequest.provider_id`, `AiProviderKind` con `#[default] OpenAi`, y
  `LOCAL_API_KEY` en `RigGateway` (proveedor con `base_url` y sin clave envía
  `"not-needed"`; sin clave y sin endpoint → `AiError::MissingKey`).
- `ai/mock.rs`: `MockGateway::with_providers(...)` para sembrar el catálogo
  en tests.
- `ai_completion.rs`: `resolve_target` convierte `params.model` en un
  `Target { provider_id, provider, model, base_url }` con la cascada
  documentada en `ARCHITECTURE/BACKEND.md` (`::` → catálogo vacío → match de
  modelo → kind legacy → proveedor único/primero con clave). El split es en
  el **primer** `::`, así `llama3.1:70b` sobrevive.
- `lib.rs`: `ai_key_status` / `ai_set_key` / `ai_clear_key` sustituidos por
  `ai_providers`, `ai_save_provider`, `ai_remove_provider`,
  `ai_clear_provider_key`; los cuatro devuelven `Vec<ProviderStatus>`.

### Frontend (`src/`)

- `src/lib/utils/ai-config.svelte.ts` (nuevo): espejo reactivo del catálogo
  (`aiProviders`, `ProviderStatus`, `ProviderDraft`, `refreshAiProviders`,
  `ensureAiProviders`, `setAiProviders`, `providerName`, `PROVIDER_KINDS`,
  `KIND_LABELS`), mismo patrón que `blueprint-context.svelte.ts`.
- `AiKeysDialog.svelte` → `AiProvidersDialog.svelte` (`git mv`, mantiene el
  historial) convertido en CRUD: alta/edición/borrado de proveedor, `Base
  URL`, lista de modelos (uno por línea), campo de clave siempre vacío
  (vacío = mantener la guardada) y *Clear key*.
- `Toolbar.svelte`: botón **AI Keys** → **AI Providers**.
- `catalog.ts`: nuevo `ParamField` `{ kind: 'model' }`; `ai.completion`
  declara solo `model`, `system`, `concurrency`, … (ya no `provider` ni
  `base_url`).
- `Inspector.svelte`: renderiza `kind: 'model'` como `<select>` de
  `<optgroup>` (uno por proveedor) con valor `providerId::model`, opción
  `... · not in the list` para un valor guardado que ya no está en el catálogo,
  y text input con placeholder `providerId::model` cuando no hay proveedores
  configurados. El guardado hace `params: { ...d.params, [key]: value }`, así
  que se conservan los params legacy que el catálogo ya no declara.

### Documentación

- `ARCHITECTURE/decisions.md`: ADR-003 reescrito y retitulado («Los
  proveedores de IA y sus claves nunca viven en el blueprint»); ADR-005 nuevo
  («Catálogo global de proveedores; el nodo solo elige modelo»). IDs previos
  sin renumerar.
- `SPEC/decisions.md`: FD-002 nuevo (comportamiento visible del diálogo y del
  desplegable de modelo).
- `ARCHITECTURE/BACKEND.md`: mapa de `ai/*`, tabla de comandos Tauri (4
  `ai_*`), snippet real de `pub fn run()`, sección `## ai/` reescrita con
  `secrets.rs`, la cascada de `resolve_target` y `LOCAL_API_KEY`.
- `ARCHITECTURE/FRONTEND.md`: filas de Toolbar / Inspector /
  `AiProvidersDialog` / `ai-config.svelte.ts`, `ParamField` con `textarea` y
  `model`, `CATEGORY_COLORS` y `CATEGORIES` reales (8 categorías), secciones
  nuevas para `ai-config.svelte.ts` y `AiProvidersDialog.svelte`, y el
  comportamiento del `<select>` de modelo en el Inspector.
- `changelog/0.1.0.md`: `Added` / `Changed` / `Known issues` actualizados.
- `docs/progress/2026-11-13-ai-provider-catalog.md`: este informe.
- `docs/examples/AI-blueprint.json` se conserva **intacto** a propósito: es
  un blueprint real en forma legacy (`provider`, `model`, `base_url`) y sirve
  como ejemplo de compatibilidad.

## Current State & Verified Behavior

Comandos ejecutados y resultados reales:

```text
$ cd src-tauri && cargo test --lib -- --skip file_iterate_folder
test result: ok. 255 passed; 0 failed; 0 ignored; 0 measured; 5 filtered out; finished in 1.00s

$ pnpm check
svelte-check found 0 errors and 3 warnings in 2 files
```

- Los 3 warnings de `svelte-check` son preexistentes y están en ficheros no
  tocados: `src/lib/components/Palette.svelte:59:15`,
  `src/routes/dashboard/+page.svelte:419:7` (×2).
- Las 5 advertencias de `cargo` son preexistentes y apuntan a ficheros no
  tocados: `Blueprint::empty` (`src/blueprint.rs:125`),
  `EvalCtx::debug|info|warn|error` (`src/executor/mod.rs:62`),
  `ExecError::MissingInput` (`src/executor/mod.rs:170`),
  `InputShape::is_empty` (`src/executor/mod.rs:364`) y un doc comment sin
  usar (`src/executor/runner.rs:198`). Ninguna proviene de los ficheros de
  esta entrega (`SecretStore::path` sigue bajo `#[allow(dead_code)]`).
- `--skip file_iterate_folder` excluye los 5 tests de ese nodo; 3 de ellos
  fallan siempre (`Not a directory (os error 20)` — PROD-TECH-007). Son ajenos
  a este cambio.
- Tests en Rust: `ai/secrets.rs` pasa de 5 a 9 (`generated_ids_do_not_collide`,
  `keyless_provider_needs_a_base_url`, `legacy_kind_map_is_migrated` y
  `provider_infos_hide_the_key_but_show_has_key` son nuevos; los otros 5
  quedaron reescritos sobre el almacén de registros). `ai_completion.rs` pasa
  de 16 a 24; los 8 nuevos son
  `provider_id_in_the_model_selects_that_provider`,
  `a_configured_model_name_selects_its_provider`,
  `legacy_params_still_run_with_an_empty_catalog`,
  `unknown_provider_id_errors_before_calling_out`,
  `provider_param_without_a_matching_provider_errors`,
  `several_providers_resolve_to_a_configured_one`,
  `legacy_base_url_wins_unless_the_provider_is_named`,
  `blank_model_falls_back_to_the_kind_default`. El test preexistente
  `unknown_provider_errors_before_calling_out` es lo que obliga a conservar el
  mensaje legacy `unknown AI provider 'x' (expected one of: openai,
  anthropic)` cuando no hay proveedores configurados.
- `grep` de `AI Keys|ai_key_status|AiKeysDialog|SecretStatus|ai_set_key|ai_clear_key`
  sobre `src/`, `src-tauri/` y `docs/`: solo aparece en el bloque `Changed`
  del changelog (deliberado) y en `docs/progress/2026-11-11-ai-integration.md`
  (registro histórico, no reescrito).

## Problems & Blockers

Sin bloqueos. Riesgos y límites conocidos:

- **No verificable offline**: el envío de `LOCAL_API_KEY` (`"not-needed"`) a
  un endpoint local que sí autentique reportaría un error de autenticación en
  lugar del `missing API key` anterior. Registrado como *Known issue* en el
  changelog; solo se comprueba contra un proveedor real al ejecutar la app.
- La lista `models` de un proveedor es orientativa: no hay descubrimiento
  (`/v1/models`), y un nodo puede escribir cualquier modelo tras `providerId::`.
  Está en *Known issues*.
- Un blueprint con `ollama::llama3.1:70b` requiere en otra máquina un
  proveedor con ese mismo `id`, o re-elegir modelo en el Inspector. Es
  consecuencia aceptada de FD-002 / ADR-005 y está documentada.
- Varias filas de la tabla de ficheros de `ARCHITECTURE/FRONTEND.md` siguen
  con conteos de LoC desactualizados de entregas previas; solo se corrigieron
  las filas tocadas por este cambio.

## Next Steps

- Probar el flujo completo en la app real: alta de un proveedor Ollama local
  sin clave, selección desde el Inspector, y un `ai.completion` contra
  `docs/examples/AI-blueprint.json` (forma legacy) para confirmar la
  compatibilidad end-to-end.
- Considerar descubrimiento de modelos (`GET /v1/models`) desde el diálogo,
  o un botón «probar proveedor», si el catálogo manual resulta incómodo.
- Renombrar proveedores sigue sin estar soportado a propósito (el `id` es el
  handle que guardan los blueprints). Si se pide, hará falta reescribir los
  blueprints que lo referencian.
- PROD-TECH-007 (`file_iterate_folder`) sigue pendiente y es ajeno a esta
  entrega.
