# Blueprint Processor — Documentación del Proyecto

Bienvenido a la documentación de **Blueprint Processor**, una aplicación de escritorio para diseñar, ejecutar y visualizar pipelines modulares de procesamiento de datos (blueprints) mediante una interfaz visual.

## 📖 Navegación rápida

| Documento | Contenido | Para quién |
|-----------|-----------|------------|
| [`SPEC.md`](SPEC.md) | Visión del producto, principios operacionales, modelo de datos y flujo funcional | Cualquiera que necesite entender QUÉ hace la aplicación y POR QUÉ |
| [`ARCHITECTURE.md`](ARCHITECTURE.md) | Arquitectura general, diagrama de capas, invariantes clave, decisiones técnicas y glosario | Nuevos contributors, revisores |
| [`ARCHITECTURE/BACKEND.md`](ARCHITECTURE/BACKEND.md) | Recorrido detallado del código Rust: structs, helpers, executors por nodo, tests | Contributors Rust / Backend |
| [`ARCHITECTURE/FRONTEND.md`](ARCHITECTURE/FRONTEND.md) | Recorrido detallado del código Svelte/TS: componentes, tipos, utilidades, flujos | Contributors Svelte / Frontend |
| [`ARCHITECTURE/recipes.md`](ARCHITECTURE/recipes.md) | Cookbook de "cómo extiendo X" — añadir nodos, params, port types, Tauri commands | Cualquiera que extienda la app |
| [`ARCHITECTURE/decisions.md`](ARCHITECTURE/decisions.md) | Decisiones técnicas y de seguridad (ADR) | Contributors, revisores |
| [`ARCHITECTURE/PLUGINS.md`](ARCHITECTURE/PLUGINS.md) | Propuesta (no implementada) de plugins de nodos: opciones evaluadas, diseño y plan por fases | Quien quiera extender la app sin recompilar |
| [`TASKS.md`](TASKS.md) | Tareas pendientes y en progreso con estados | Todo el equipo |

## 🗂️ Estructura de `docs/`

```text
docs/
├── README.md              ← esta página (punto de entrada)
├── SPEC.md                # especificación del producto
├── ARCHITECTURE.md        # arquitectura general + decisiones técnicas
├── ARCHITECTURE/          # documentación técnica complementaria
│   ├── BACKEND.md         # recorrido del fuente Rust
│   ├── FRONTEND.md        # recorrido del fuente Svelte/TS
│   ├── PLUGINS.md         # propuesta de plugins de nodos (sin implementar)
│   └── recipes.md         # cookbook de mantenimiento y extensión
├── TASKS.md               # tareas pendientes
├── changelog/             # registro de cambios por versión
└── progress/              # informes de progreso por hito
```

## 🔗 Cross-references entre documentos

- **Product vision** → [`SPEC.md`](SPEC.md)
- **Arquitectura general** → [`ARCHITECTURE.md`](ARCHITECTURE.md)
- **Detalles técnicos Rust** → [`ARCHITECTURE/BACKEND.md`](ARCHITECTURE/BACKEND.md)
- **Detalles técnicos Svelte/TS** → [`ARCHITECTURE/FRONTEND.md`](ARCHITECTURE/FRONTEND.md)
- **Cómo extender la app** → [`ARCHITECTURE/recipes.md`](ARCHITECTURE/recipes.md)
- **Plugins de nodos (propuesta)** → [`ARCHITECTURE/PLUGINS.md`](ARCHITECTURE/PLUGINS.md)
- **Tareas activas** → [`TASKS.md`](TASKS.md)
