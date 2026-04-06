# @animaOS-SWARM/swarm

TypeScript swarm coordination primitives for animaOS-SWARM.

This package exports `SwarmCoordinator`, `MessageBus`, the built-in supervisor, dynamic, and round-robin strategies, plus the `swarm()` helper for constructing swarm configs.

The Rust workspace owns canonical production execution, but this package remains useful for shared types, local orchestration utilities, and compatibility tests.

## Build

Run `bun x nx build @animaOS-SWARM/swarm`.

## Test

Run `bun x nx test @animaOS-SWARM/swarm`.
