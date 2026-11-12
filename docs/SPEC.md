# 🚀 Blueprint Processor - Product Specification (SPEC)

Este documento define la visión, los principios operativos y las decisiones funcionales que rigen el procesamiento de blueprints. Es la fuente de verdad sobre **QUÉ** debe hacer la aplicación para cumplir con su objetivo de usuario final.

## 🎯 Objetivo Principal
Permitir a los usuarios diseñar, ejecutar y visualizar pipelines modulares de procesamiento de datos (blueprints) mediante una interfaz visual intuitiva en un entorno de escritorio robusto y sandboxed.

## ✨ Principios Operacionales del Blueprint
1. **Composición Visual:** El sistema se basa en la composición de nodos (componentes funcionales) conectados por aristas que representan flujos de datos.
2. **Ejecución Determinista:** La ejecución siempre sigue un orden topológico fijo, garantizando resultados predecibles sin importar el orden visual del usuario.
3. **Separación de Preocupaciones:** La UI (SvelteKit) se encarga de la composición y la interacción; el motor Rust sandboxed se encarga exclusivamente de la lógica de procesamiento y el manejo seguro de datos.
4. **Experiencia Guiada:** El sistema debe guiar al usuario final, minimizando la exposición a conceptos técnicos como `Kahn sort`, `Tauri invoke` o `serde_json::Value`.

## 💾 Modelo de Datos (El Blueprint)
Un blueprint es un artefacto serializable que encapsula:
*   **Metadata (`BlueprintMeta`):** Información sobre el proceso (nombre, descripción, autores, tags).
*   **Nodos:** La colección de componentes funcionales. Cada nodo debe declarar explícitamente sus puertos de entrada y salida.
*   **Conexiones (Edges):** Los enlaces entre nodos, que definen las dependencias y el flujo de datos.

## 🚦 Flujo Funcional de Ejecución (Run Flow)
1. **Input:** El usuario define los *inputs externos* al grafo en la UI (`io.input` nodes). Estos son valores iniciales del sistema o cargados desde un archivo CSV/JSON.
2. **Build Graph:** La aplicación mapea visualmente los nodos y conexiones a un Grafo Acíclico Dirigido (DAG).
3. **Execution Core:** El motor Rust (`run_graph` en `executor/runner.rs`) ejecuta el grafo:
    *   Calcula el orden topológico de ejecución.
    *   Itera sobre los nodos, recolectando inputs desde los outputs previamente calculados por sus predecesores y consumiendo *external_inputs*.
    *   El resultado final se compila en un objeto `{ outputs, logs, stats }` que regresa al frontend.

---

**Referencias de Profundidad Técnica:**
- **Arquitectura general del sistema:** Ver [`ARCHITECTURE.md`](ARCHITECTURE.md)
- **Detalles Backend (Rust):** Ver [`ARCHITECTURE/BACKEND.md`](ARCHITECTURE/BACKEND.md)
- **Detalles Frontend (Svelte/TS):** Ver [`ARCHITECTURE/FRONTEND.md`](ARCHITECTURE/FRONTEND.md)
- **Recetas de extensión:** Ver [`ARCHITECTURE/recipes.md`](ARCHITECTURE/recipes.md)
