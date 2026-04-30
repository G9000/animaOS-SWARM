# @animaOS-SWARM/memory

TypeScript memory primitives for animaOS-SWARM.

This package provides BM25 search, task history, document ingestion, memory management helpers, and the TypeScript memory plugin/provider surface used by local workflows and compatibility layers.

The reusable Rust memory implementation now lives in `packages/core-rust/crates/anima-memory`, while `hosts/rust-daemon` is one runnable host that uses the reusable Rust core. This package remains useful for shared utilities, tests, and local tooling.

Current memory coverage includes:

- BM25 ranking utilities for local semantic-ish lookup
- task-history indexing and retrieval helpers
- memory storage, filtering, persistence, and search through `MemoryManager`
- first-class agent-to-agent relationship edges with evidence memory IDs
- provider, evaluator, and plugin glue for TypeScript-side memory workflows

## Quick Example

```ts
import { MemoryManager } from '@animaOS-SWARM/memory';

const manager = new MemoryManager();

manager.add({
  agentId: 'agent-1',
  agentName: 'manager',
  type: 'fact',
  content: 'Launch recovered after a /health recheck.',
  importance: 0.8,
  scope: 'private',
  tags: ['launch', 'health'],
});

const results = manager.search('launch health');
console.log(results[0]?.content);
```

Memories can also be scoped with `scope: 'shared' | 'private' | 'room'` plus optional `roomId`, `worldId`, and `sessionId` filters. If a new memory omits `scope`, the manager defaults it to `room` when `roomId` is present and `private` otherwise.

Agent relationships are stored separately from memories:

```ts
manager.upsertAgentRelationship({
  sourceAgentId: 'planner',
  sourceAgentName: 'Planner',
  targetKind: 'user',
  targetAgentId: 'user-1',
  targetAgentName: 'Leo',
  relationshipType: 'responds_to',
  summary: 'Planner answered Leo during launch planning.',
  strength: 0.65,
  confidence: 0.75,
  evidenceMemoryIds: ['mem-123'],
  worldId: 'world-1',
});

manager.upsertAgentRelationship({
  sourceAgentId: 'planner',
  sourceAgentName: 'Planner',
  targetAgentId: 'critic',
  targetAgentName: 'Critic',
  relationshipType: 'collaborates_with',
  summary: 'Critic pressure-tests Planner before launch decisions.',
  strength: 0.85,
  confidence: 0.75,
  evidenceMemoryIds: ['mem-123'],
  worldId: 'world-1',
});

const userEdges = manager.listAgentRelationships({ entityId: 'user-1', targetKind: 'user' });
const agentEdges = manager.listAgentRelationships({ agentId: 'critic', worldId: 'world-1' });
```

This is the graph foundation for agent-to-agent and agent-to-user memory: structured directed edges plus memory evidence, not just conventions embedded in `content`.

## Build

Run `bun x nx build @animaOS-SWARM/memory`.

## Test

Run `bun x nx test @animaOS-SWARM/memory`.
