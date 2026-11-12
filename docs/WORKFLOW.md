# Workflow — Cómo trabajar en Blueprint Processor

Este documento define las convenciones de trabajo para contribuir al proyecto. Sigue las reglas globales del repositorio (`AGENTS.md`) como base; aquí se especifican los detalles propios del proyecto.

## 🚀 Empezar un trabajo

1. **Lee el contexto relevante antes de tocar código:**
   - Para entender QUÉ hace la app: [`SPEC.md`](SPEC.md)
   - Para entender CÓMO está construida: [`ARCHITECTURE.md`](ARCHITECTURE.md)
   - Para detalles del backend Rust: [`ARCHITECTURE/BACKEND.md`](ARCHITECTURE/BACKEND.md)
   - Para detalles del frontend Svelte: [`ARCHITECTURE/FRONTEND.md`](ARCHITECTURE/FRONTEND.md)

2. **Consulta `TASKS.md`** para ver el estado de las tareas activas y evitar duplicar trabajo.

3. **Para tareas complejas**, crea una rama dedicada (`task/{id}-{slug}`) antes de empezar.

## 📝 Convenciones de código

### Rust (Backend)
- Un fichero por kind de nodo en `src-tauri/src/executor/nodes/<kind_snake>.rs`
- Cada fichero de nodo debe tener su propio `#[cfg(test)] mod tests` con al menos un test positivo
- Usa los helpers de `nodes::test_helpers` (`ctx_with`, `empty_inputs`) para tests
- Los executors son funciones puras: `pub fn exec(inputs, params, ctx) -> Result<NodeOutputs, ExecError>`
- No uses `Box<dyn Trait>` para executors — usa function pointers (`NodeExec = fn(...)`)

### TypeScript / Svelte (Frontend)
- El catálogo de nodos en `src/lib/nodes/catalog.ts` es la fuente única de verdad para qué nodos puede colocar el usuario
- Los tipos TS en `src/lib/types/blueprint.ts` DEBEN mantenerse en sync con las structs Rust en `src-tauri/src/blueprint.rs`
- Usa `$state` para estado observable fuera del canvas; usa writables solo para `nodes`/`edges` de Svelte Flow
- Lee los mirrors `$state` (`flowNodes`/`flowEdges`) desde componentes derivados, no te suscribas a los writables directamente

## 🧪 Tests

### Backend (Rust)
```bash
cd src-tauri && cargo test --lib              # todos los tests
cd src-tauri && cargo test --lib executor::   # solo el crate executor
```

### Frontend (Svelte/TS)
```bash
pnpm check          # svelte-check en todo el workspace
```

No hay tests del lado Svelte hoy. El frontend se verifica por `pnpm check` + smoke testing manual con `pnpm tauri dev`.

## 📦 Build y ejecución

```bash
# Desarrollo completo (frontend + shell Tauri)
pnpm tauri dev

# Solo frontend (sin Tauri, para iteración rápida)
pnpm dev

# Build de producción
pnpm build
```

## ✅ Completar una tarea

1. **Verifica** que los tests pasan (`cargo test --lib` + `pnpm check`)
2. **Actualiza la documentación** afectada por los cambios
3. **Actualiza `TASKS.md`**: marca la tarea como completada y añádela al changelog
4. **Crea un commit** con mensaje `{task-id}: {brief description}` (solo los cambios de la tarea)

## 📚 Escribir documentación

- Las decisiones funcionales (comportamiento visible para el usuario) van en [`SPEC/decisions.md`](SPEC/decisions.md)
- Las decisiones técnicas y de seguridad van en [`ARCHITECTURE/decisions.md`](ARCHITECTURE/decisions.md) — crear cuando sea necesario
- Los informes de progreso para hitos importantes van en `docs/progress/YYYY-MM-DD-topic.md`
- Nunca almacenes secretos ni credenciales en la documentación

## 🔄 Extender la aplicación

Para añadir cualquier funcionalidad nueva, consulta [`ARCHITECTURE/recipes.md`](ARCHITECTURE/recipes.md) que contiene recetas paso a paso para:
- Añadir un nuevo node kind
- Añadir un nuevo ParamField editable
- Añadir un nuevo port type
- Añadir un nuevo Tauri command
- Bumpear la versión del schema
