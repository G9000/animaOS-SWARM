# @animaOS-SWARM/core

Shared TypeScript contracts, plugin types, and compatibility utilities for the CLI, SDK, TUI, and UI.

This is the TypeScript core port under `packages/core-ts`. The package name stays `@animaOS-SWARM/core` so existing imports do not churn.

It is not the source of truth for runtime execution. The reusable Rust runtime core lives in `packages/core-rust`, and the current runnable Rust host lives in `hosts/rust-daemon`.

Current core coverage includes:

- shared types for agents, plugins, models, components, and swarm event payloads
- lightweight runtime-port support such as the event bus and agent-runtime helpers used by TypeScript surfaces
- adapter compatibility helpers used by TypeScript-side provider integrations
- shared daemon health messaging used by both the CLI and TUI launch flows
- ergonomic config builders like `agent()`, `plugin()`, and `action()`

## Quick Example

```ts
import {
  EventBus,
  agent,
  describeDaemonWarningTransition,
} from '@animaOS-SWARM/core';

const eventBus = new EventBus();
const manager = agent({
  name: 'manager',
  model: 'gpt-5.4',
});

await eventBus.emit(
  'agent:spawned',
  { agentId: 'launch:manager', name: manager.name },
  'launch:manager'
);

const transition = describeDaemonWarningTransition(
  'daemon unavailable',
  null,
  'manual'
);

console.log(transition.message);
// "Daemon reachable again. Launch tasks can run."
```

## Build

Run `bun x nx build @animaOS-SWARM/core`.

## Test

Run `bun x nx test @animaOS-SWARM/core`.

## API Reference

See [API.md](./API.md) for the full internal type reference. Public consumers should use [`@animaOS-SWARM/sdk`](../sdk) instead.
