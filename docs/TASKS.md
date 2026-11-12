# Tasks Pending (Blueprint Processor)

Este documento rastrea las tareas de desarrollo y mejora para la plataforma Blueprint Processor, siguiendo la estructura definida en el proyecto.

## 🟢 Tareas de Lanzamiento a Producción (Producción V1.0)
Estas tareas se centran en convertir un MVP funcional en un producto robusto y usable por usuarios finales.

- [ ] PROD-UX-001 - Implementar documentación exhaustiva para usuario final - [Critical, UX]
  Creación de una guía paso a paso separada de la documentación técnica (docs/) que enseñe al usuario qué hacer ("Cómo construir un pipeline X") en lugar de explicar cómo funciona el motor.
  Affected systems:
  - Frontend (SvelteKit)
  - Docs/

- [ ] PROD-UX-002 - Flujo de Onboarding y Tutorial Guiado - [High, UX]
  Desarrollar un tour inicial para nuevos usuarios que guíe por la interfaz principal (Palette -> Editor -> Run), explicando el concepto central de los blueprints.
  Affected systems:
  - Frontend (SvelteKit)
  - src/lib/components/

- [ ] PROD-UX-003 - Gestión Amigable de Errores en Ejecución - [Critical, Robustness]
  Mejorar el manejo de errores del motor Rust para que los fallos de nodos sean reportados al usuario final indicando: 1) El nodo exacto que falló, 2) Por qué (Ej. "Input esperado: número, recibido: texto"), y 3) Cómo corregirlo.
  Affected systems:
  - Backend (Rust - runner.rs)
  - Frontend (SvelteKit)

- [ ] PROD-TECH-004 - Cobertura de Pruebas de Integración Avanzadas - [High, Testing]
  Aumentar la suite de pruebas `cargo test` para simular flujos complejos con 5+ nodos encadenados y diversas interacciones de datos (tipos incompatibles).
  Affected systems:
  - Backend (Rust)

- [ ] PROD-TECH-005 - Validaciones Estáticas de Conexión en UI - [Medium, Robustness]
  Implementar validaciones en tiempo real en la interfaz (Svelte Flow): prevenir visualmente o mediante advertencias la conexión de puertos con tipos incompatibles (`PortDataType`), antes incluso de ejecutar el blueprint.
  Affected systems:
  - Frontend (SvelteKit)

- [ ] PROD-TECH-006 - Optimización y Profiling de Rendimiento en I/O Masivo - [Medium, Performance]
  Realizar *profiling* del motor Rust bajo cargas pesadas de datos (`dataset.csv` con millones de registros) para identificar cuellos de botella de memoria o CPU.
  Affected systems:
  - Backend (Rust)

- [ ] PROD-TECH-007 - Corregir tests de `file.iterate` y unificar `io.input` - [Medium, Testing]
  `cargo test --lib` falla en tres tests de `executor::nodes::file_iterate_folder`: pasan una ruta de *fichero* a `fs::read_dir`, que espera un directorio (`Not a directory (os error 20)`). Además ese fichero implementa `file.iterate` pero su cabecera y sus tests describen `file.read`, y el executor hace `unwrap()` sobre `params.path`. Fallos presentes ya antes de la integración de IA.
  Affected systems:
  - Backend (Rust - executor/nodes/file_iterate_folder.rs)
