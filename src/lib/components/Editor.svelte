<!--
  Svelte Flow editor canvas.

  Svelte Flow v0.1.x requires `nodes` and `edges` to be writable Svelte
  stores (Writable<Node[]>/Writable<Edge[]>). The libraries updates them
  in place to track drag/move/select. We lift the stores up to the page
  so persistence (Save) can read a snapshot via `get(...)`, and so the
  Toolbar can subscribe for status-bar counts.

  We listen to:
    onConnect  → append a new edge to the writable.
    onDelete   → clear selection if the selected node went away.
    onDrop     → spawn a node from the palette payload.
-->
<script lang="ts">
  import {
    SvelteFlow,
    Background,
    Controls,
    MiniMap,
    ConnectionMode,
    useSvelteFlow,
    type Connection,
    type IsValidConnection,
    type Node as SFNode,
  } from '@xyflow/svelte';
  import { onMount } from 'svelte';
  import { get } from 'svelte/store';
  import type { Writable } from 'svelte/store';
  import '@xyflow/svelte/dist/style.css';

  import { nodeTypes } from './nodes';
  import {
    edgeFromConnection,
    makeFlowNode,
    type FlowEdge,
    type FlowNode,
  } from '$lib/utils/blueprint-io';
  // Per-node projection of the edges list: `nodeId → Set<portId>`.
  // `BlueprintNode.svelte` reads this to disable the inline value
  // editor on a port whenever it's wired. Mirroring the projection
  // here (vs letting every node subscribe to the full edges list)
  // keeps the re-render scope per-edit small even on large graphs.
  import { recomputeWiredPortKeys } from '$lib/utils/blueprint-context.svelte';

  interface Props {
    nodes: Writable<FlowNode[]>;
    edges: Writable<FlowEdge[]>;
  }

  let { nodes, edges }: Props = $props();

  const { screenToFlowPosition } = useSvelteFlow();

  // Mirror the edges store into a `nodeId → Set<portId>` projection
  // so `BlueprintNode.svelte` can ask "is THIS port wired?" cheaply
  // without subscribing to the full edges list. Triggered on every
  // edge mutation (add / delete / rewire); inside BlueprintNode the
  // per-node set lookup is wrapped in `$derived` so a single node's
  // membership re-evaluates only when its own slice changes.
  $effect(() => {
    recomputeWiredPortKeys($edges);
  });

  // WebView2/Chromium requires BOTH dragenter and dragover to call
  // preventDefault for the OS drop cursor to flip from "forbidden" to
  // "copy". Without a dragenter handler the very first time the cursor
  // enters the canvas, the browser locks the drop-target state to
  // 'none' and you get the prohibited-sign cursor.
  function onDragEnter(event: DragEvent) {
    event.preventDefault();
    if (event.dataTransfer) event.dataTransfer.dropEffect = 'copy';
  }

  function onDragOver(event: DragEvent) {
    event.preventDefault();
    if (event.dataTransfer) event.dataTransfer.dropEffect = 'copy';
  }

  function onDrop(event: DragEvent) {
    event.preventDefault();
    const payload = event.dataTransfer?.getData('application/x-blueprint-node');
    if (!payload) return;
    try {
      const { kind } = JSON.parse(payload) as { kind: string };
      const flowPos = screenToFlowPosition({
        x: event.clientX,
        y: event.clientY,
      });
      const fresh = makeFlowNode(kind, flowPos);
      nodes.update((prev: FlowNode[]) => [...prev, fresh]);
    } catch {
      // ignore malformed payload
    }
  }

  function minimapNodeColor(node: SFNode): string {
    const data = (node.data ?? {}) as { kind?: unknown };
    const kind = String(data.kind ?? '');
    if (kind.startsWith('io.')) return '#f59e0b';
    if (kind.startsWith('math.')) return '#06b6d4';
    if (kind.startsWith('string.')) return '#22c55e';
    if (kind.startsWith('array.')) return '#ec4899';
    if (kind.startsWith('literal.')) return '#a855f7';
    return '#9ca3af';
  }

  /**
   * Look up a port by id on a node and return its declared `type`.
   * Returns `undefined` when either the node or the port id is
   * missing — the caller treats that as "unknown shape" and lets
   * the permissive path through (matching today's behaviour for
   * any in-flight connection whose endpoints aren't yet
   * catalogued — e.g. an auto-inserted stub during a rename or a
   * brand-new node dragged in mid-drag).
   */
  function portType(
    nodeId: string,
    portId: string | null | undefined,
    isSource: boolean,
  ): string | undefined {
    if (!portId) return undefined;
    const allNodes = get(nodes);
    const node = allNodes.find((n) => n.id === nodeId) as
      | {
          inputs?: Array<{ id: string; type?: string }>;
          outputs?: Array<{ id: string; type?: string }>;
        }
      | undefined;
    if (!node) return undefined;
    const ports = isSource ? node.outputs ?? [] : node.inputs ?? [];
    const port = ports.find((p) => p.id === portId);
    return port?.type;
  }

  /**
   * Validator for in-progress Svelte Flow drag-and-drop
   * connections. Called every time the user drags a wire from
   * a source handle to a candidate target handle; returns `true`
   * iff the drop is allowed.
   *
   * **Required rules** (covered in this implementation):
   *
   *   1. **No self-loops** — a node never wires to itself.
   *   2. **`then_in` ↔ `then_out` only** — the execution
   *      (`exec`) ports are the topological-ordering pins
   *      Unreal/Godot/Blender call "white chevrons" / "exec
   *      tokens". They carry NO data; they exist purely to
   *      sequence nodes whose dataflow otherwise has no
   *      dependency. Wiring `then_out` → a data input would
   *      silently feed `Null` into the consumer (probably
   *      ignored, possibly erroring on a strict type); wiring a
   *      data output → `then_in` is even worse — the data
   *      value lands in the order-token slot and is treated as
   *      an unconditional "ready" signal regardless of its
   *      actual semantics. Both are user typos with no
   *      recovery path other than "look at the runner output
   *      and see why nothing fired". Force exec↔exec at
   *      compose-time so the rejection is visible (the line
   *      snaps back, the cursor turns red) instead of becoming
   *      a silent deadlock at run-time.
   *   3. **At-most-one wire per target input** — re-routing is
   *      the common flow ("I had A → X, now I want B → X").
   *      Enforced inside `handleConnect` (prior edges to the
   *      same `(target, targetHandle)` are dropped before the
   *      new one is appended). Not a validator return-value
   *      concern — Svelte Flow itself rejects re-attaching a
   *      second wire to a handle that's already occupied, so
   *      this validator stays silent on the matter and lets
   *      the alternative sink/list shape do the work.
   */
  /**
   * Rule 2 of the connection validator — rejection guard for
   * mismatched exec↔data wiring.
   *
   * An input THEN port (`id: 'then_in'`, type `exec`) may ONLY
   * wire to another node's THEN output (`id: 'then_out'`, also
   * type `exec`). Wiring across the boundary in either direction
   * is rejected at compose-time so a user typo becomes a snap-
   * back at the validator instead of a silent runner deadlock:
   *
   *   * `then_out` → data input   (src exec, dst data):
   *     The exec token lands in a data slot and silently feeds
   *     `Null` into the consumer — either ignored or, on a
   *     strict type, an error during a real run. Both outcomes
   *     waste a debugging cycle.
   *   * data output → `then_in`   (src data, dst exec):
   *     The data value lands in the order-token slot and is
   *     treated as an unconditional "ready" signal regardless
   *     of its actual semantics — a silent executor-level
   *     deadlock.
   *
   * Unknown shape on EITHER side (the `portType` lookup returned
   * `undefined` because the port id isn't in the catalogue yet,
   * e.g. an auto-inserted stub during a rename or a fresh node
   * dragged in mid-drag) is treated as permissive and lets the
   * validator pass through — preserving today's behaviour for
   * in-flight connections whose endpoints aren't yet resolved.
   *
   * @returns `true` when both ports agree on the shape
   *   (legitimate connection); `false` ONLY on the cross-
   *   shape mismatch described above.
   */
  function isExecToExecConnection(
    sourceType: string | undefined,
    targetType: string | undefined,
  ): boolean {
    if (sourceType === undefined || targetType === undefined) return true;
    const sourceIsExec = sourceType === 'exec';
    const targetIsExec = targetType === 'exec';
    // Shapes-must-agree — XNOR semantics (NOT XOR). Returns
    // `false` ONLY on the cross-shape rejection case described
    // in the doc-block above. The previous inline validator used
    // XOR as the REJECTION predicate (`!==`); the helper inverts
    // to XNOR (`===`) which reads more naturally as "both ports
    // share a shape". Logically equivalent — a refactor of the
    // old code's `if (sourceIsExec !== targetIsExec) return false`
    // would land here.
    return sourceIsExec === targetIsExec;
  }

  const isValidConnection: IsValidConnection = (edgeOrConn) => {
    const conn = edgeOrConn as { source: string; target: string; sourceHandle?: string | null; targetHandle?: string | null };
    if (conn.source === conn.target) return false;
    if (!conn.sourceHandle || !conn.targetHandle) return false;
    const sourceType = portType(conn.source, conn.sourceHandle, true);
    const targetType = portType(conn.target, conn.targetHandle, false);
    // **Rule 2.** `then_in` may only wire to another node's
    // `then_out` — exec↔exec only, never to or from a data port.
    // See the doc-block on `isExecToExecConnection` for the
    // rationale AND the permissive unknown-shape edge case.
    if (!isExecToExecConnection(sourceType, targetType)) return false;
    return true;
  };

  function handleConnect(conn: Connection) {
    edges.update((prev: FlowEdge[]) => {
      // Drop any prior edge pointing at the same input — connecting to
      // an already-connected target REPLACES the old link, not stacks
      // a parallel one. (Connectors and `array.create` targets both
      // expose a single input handle, so even multi-port nodes get the
      // right behaviour automatically.)
      const filtered = prev.filter(
        (e) =>
          e.target !== conn.target ||
          (e.targetHandle ?? '') !== (conn.targetHandle ?? ''),
      );
      return [...filtered, edgeFromConnection(conn)];
    });
  }

  // Defensive click-to-spawn fallback. Listens for a window-level event
  // dispatched by Palette when the user clicks (instead of drags) a node.
  // Inserts the new node at the centre of the visible canvas so it always
  // ends up on screen, regardless of whether Tauri/WebView allow HTML5 D&D.
  onMount(() => {
    const handler = (event: Event) => {
      const ce = event as CustomEvent<{ kind: string }>;
      const kind = ce.detail?.kind;
      if (!kind) return;
      const wrap = document.querySelector('.canvas-wrap') as HTMLElement | null;
      const rect = wrap?.getBoundingClientRect();
      const cx = rect ? rect.left + rect.width / 2 : window.innerWidth / 2;
      const cy = rect ? rect.top + rect.height / 2 : window.innerHeight / 2;
      const flowPos = screenToFlowPosition({ x: cx, y: cy });
      const fresh = makeFlowNode(kind, flowPos);
      nodes.update((prev: FlowNode[]) => [...prev, fresh]);
    };
    window.addEventListener('blueprint:add-node', handler as EventListener);
    return () => window.removeEventListener('blueprint:add-node', handler as EventListener);
  });
</script>

<div
  class="canvas-host"
  ondragenter={onDragEnter}
  ondragover={onDragOver}
  ondrop={onDrop}
  role="application"
>
  <SvelteFlow
    {nodes}
    {edges}
    {nodeTypes}
    connectionMode={ConnectionMode.Strict}
    {isValidConnection}
    onconnect={handleConnect}
    fitView
    snapGrid={[12, 12]}
    proOptions={{ hideAttribution: true }}
  >
    <Background bgColor="#f7f7f9" patternColor="#d4d4d8" gap={16} size={1} />
    <Controls />
    <MiniMap pannable zoomable nodeColor={minimapNodeColor} maskColor="rgba(0,0,0,0.04)" />
  </SvelteFlow>
</div>

<style>
  .canvas-host {
    flex: 1;
    min-width: 0;
    min-height: 0;
    position: relative;
    background: #f7f7f9;
  }

  :global(.canvas-host .svelte-flow) {
    background: #f7f7f9;
    --xy-edge-stroke-default: #5a5a5a;
    --xy-edge-stroke-selected-default: #2d7cf1;
    --xy-connectionline-stroke-default: #2d7cf1;
    --xy-handle-background-color-default: #888;
    --xy-handle-border-color-default: #fff;
    /* Disable Svelte Flow's default node styling: we draw our own
       selection border inside BlueprintNode, and letting the lib's
       defaults run on top produces doubled-up outlines / halos /
       a white-from-wrapper halo / hover shadow that the user kept
       flagging. Variables cleared:
       - `--xy-node-border-default`       (1px dark hairline on every
         .svelte-flow__node wrapper).
       - `--xy-node-border-selected-default` (selected-variant override).
       - `--xy-node-boxshadow-selected-default` (0.5px inset shadow on
         .selected).
       - `--xy-node-boxshadow-hover-default` (0 1px 4px 1px shadow on
         :hover — the user explicitly asked this one removed last).
       - `--xy-node-background-color-default` (#fff wrapper halo). */
    --xy-node-border-default: none;
    --xy-node-border-selected-default: transparent;
    --xy-node-boxshadow-selected-default: none;
    --xy-node-boxshadow-hover-default: none;
    --xy-node-background-color-default: transparent;
    --xy-node-border-radius-default: 0;
    --xy-node-width-default: max-content;
    --xy-node-padding-default: 0;
    --xy-node-color-default: inherit;
    --xy-selection-background-color-default: rgba(45, 124, 241, 0.08);
    --xy-selection-border-default: 1.5px dashed #2d7cf1;
    --xy-controls-button-background-color-default: #fff;
    --xy-controls-button-background-color-hover-default: #f4f4f5;
    --xy-minimap-background-color-default: rgba(255, 255, 255, 0.85);
    --xy-minimap-mask-background-color-default: rgba(0, 0, 0, 0.04);
  }

  /* Force-clear any rogue styling on the .svelte-flow__node wrapper.
     Belt-and-suspenders against Svelte Flow v0.1.39's base styles: even
     if the CSS variable overrides above get blown away by HMR bundle
     re-injection, this rule guarantees the wrapper is invisible (no bg,
     no border, no shadow, no outline, no inset frame) so my .node's
     rounded corners + white body are the ONLY visible boundary. */
  :global(.canvas-host .svelte-flow__node) {
    background: transparent !important;
    border: none !important;
    box-shadow: none !important;
    outline: none !important;
    padding: 0 !important;
    margin: 0 !important;
    border-radius: 0 !important;
  }
  :global(.canvas-host .svelte-flow__node.selected),
  :global(.canvas-host .svelte-flow__node:hover),
  :global(.canvas-host .svelte-flow__node:focus),
  :global(.canvas-host .svelte-flow__node:focus-visible) {
    background: transparent !important;
    border: none !important;
    box-shadow: none !important;
    outline: none !important;
  }

  :global(.canvas-host .svelte-flow__handle) {
    width: 10px;
    height: 10px;
    border: 1.5px solid white;
    box-shadow: 0 1px 2px rgba(0, 0, 0, 0.15);
  }

  :global(.canvas-host .svelte-flow__edge-path) {
    stroke-width: 1.8;
  }
</style>
