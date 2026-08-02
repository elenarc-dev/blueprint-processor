// Centralized registry of node components consumed by Svelte Flow.
//
// All kinds render through the same renderer (BlueprintNode), keyed under
// 'default'. The per-kind behavior is driven by `data.kind` inside the
// component itself.

import BlueprintNode from './BlueprintNode.svelte';
import type { NodeTypes } from '@xyflow/svelte';

export const nodeTypes: NodeTypes = {
  default: BlueprintNode as unknown as NodeTypes['default'],
};
