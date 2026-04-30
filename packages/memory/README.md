# @animaOS-SWARM/memory

TypeScript memory primitives for animaOS-SWARM.

This package provides BM25 search, task history, document ingestion, memory management helpers, and the TypeScript memory plugin/provider surface used by local workflows and compatibility layers.

The reusable Rust memory implementation now lives in `packages/core-rust/crates/anima-memory`, while `hosts/rust-daemon` is one runnable host that uses the reusable Rust core. This package remains useful for shared utilities, tests, and local tooling.

Current memory coverage includes:

- BM25 ranking utilities for local semantic-ish lookup
- task-history indexing and retrieval helpers
- memory storage, filtering, persistence, and search through `MemoryManager`
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

## Build

Run `bun x nx build @animaOS-SWARM/memory`.

## Test

Run `bun x nx test @animaOS-SWARM/memory`.
