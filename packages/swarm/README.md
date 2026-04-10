# @animaOS-SWARM/swarm

TypeScript swarm coordination primitives for animaOS-SWARM.

This package exports `SwarmCoordinator`, `MessageBus`, the built-in supervisor, dynamic, and round-robin strategies, plus the `swarm()` helper for constructing swarm configs.

The Rust workspace owns canonical production execution, but this package remains useful for shared types, local orchestration utilities, and compatibility tests.

Current swarm coverage includes:

- coordinator lifecycle and state tracking through `SwarmCoordinator`
- in-memory routing through `MessageBus`
- built-in supervisor, dynamic, and round-robin strategies
- the root `swarm()` helper for constructing a coordinator from config plus a model adapter

## Quick Example

```ts
import { swarm } from '@animaOS-SWARM/swarm';

const adapter = {
  provider: 'test',
  async generate() {
    return {
      content: { text: 'done' },
      usage: { promptTokens: 1, completionTokens: 1, totalTokens: 2 },
      stopReason: 'end',
    };
  },
};

const coordinator = swarm(
  {
    strategy: 'round-robin',
    manager: { name: 'manager', model: 'gpt-5.4' },
    workers: [{ name: 'worker', model: 'gpt-5.4' }],
  },
  adapter
);

const result = await coordinator.run('Summarize the launch state');
console.log(result.status);
```

## Build

Run `bun x nx build @animaOS-SWARM/swarm`.

## Test

Run `bun x nx test @animaOS-SWARM/swarm`.
