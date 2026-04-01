# animaOS Rust Execution Boundary Design

**Date:** 2026-04-02
**Status:** Approved

---

## Goal

Define the long-term execution boundary for the Rust rewrite:

- Rust owns engine execution
- TypeScript owns ecosystem reach
- the daemon becomes the only runtime boundary

This spec extends the earlier Rust engine design by making the ownership split explicit and by defining the migration order for runtime, swarm, daemon, and TypeScript clients.

---

## Decision Summary

The target architecture is:

- `anima-core` owns agent runtime logic, domain types, contracts, events, and the task loop
- `anima-swarm` owns orchestration, strategies, worker lifecycle, dispatch queues, and inter-agent coordination
- `anima-daemon` owns HTTP/SSE, registries, and process-level wiring
- TypeScript owns SDKs, CLI, plugin/tool authoring surfaces, config builders, and thin daemon clients

The current TypeScript implementation remains the reference until Rust parity is proven.

---

## Crate Responsibilities

| Crate | Responsibility | Know about HTTP? | Know about async? |
|---|---|---:|---:|
| `anima-core` | `AgentRuntime`, types, contracts, task loop, events | No | Yes, at interfaces only |
| `anima-swarm` | Coordinator, strategies, worker pool, message bus, dispatch queue, timeouts | No | Yes, `tokio` |
| `anima-daemon` | HTTP/SSE boundary, registries, wires core + swarm | Yes | Yes, `tokio` + `axum` |
| TypeScript SDK/CLI | HTTP client, plugin registration, config builders | Yes | Yes, `async/await` |

### `anima-core`

`anima-core` must stay transport-agnostic. It owns:

- `AgentRuntime`
- agent config/state
- message/content/task/event types
- model/tool/provider/evaluator contracts
- the agent execution loop
- token accounting

It must not depend on HTTP, SSE, `axum`, or daemon-specific transport concerns.

It should remain runtime-agnostic but not sync-only. Model/tool/provider/evaluator boundaries should be async-aware so the runtime can support real network-backed providers and non-blocking tool execution without forcing blocking wrappers.

### `anima-swarm`

`anima-swarm` owns async orchestration:

- swarm coordinator
- strategies
- worker pool
- message bus
- serial dispatch queue
- task timeouts and cancellation
- spawn/send/broadcast hooks
- swarm state aggregation

This crate should be built around `tokio` from the start. It should not know about HTTP routes or daemon request parsing.

### `anima-daemon`

`anima-daemon` is the process boundary:

- HTTP API
- SSE event streaming
- daemon configuration
- runtime and swarm registries
- transport validation
- wiring between `anima-core` and `anima-swarm`

It should use `axum` on top of `tokio`. It should not become the place where business logic or coordination logic accumulates.

### TypeScript

TypeScript continues to own ecosystem reach:

- SDK
- CLI
- plugin and tool authoring surface
- config builders
- external app integrations

The TypeScript layer should become a thin client to the daemon:

- HTTP for control-plane requests
- SSE for lifecycle and event streaming

TypeScript should stop embedding engine execution once Rust parity is proven.

---

## Why `tokio`

Yes, the Rust runtime boundary should use `tokio`.

Reasons:

- swarm coordination is inherently async
- model adapters will make network calls
- providers and tools may perform I/O
- daemon transport, cancellation, streaming, and timeouts are easier to express on async primitives
- `tokio` is the right foundation for long-lived worker pools and event streaming

However, `tokio` should appear only where it belongs:

- `anima-core` should be async-aware at trait boundaries, but not tied to a specific runtime
- `anima-swarm` should use `tokio` directly
- `anima-daemon` should use `tokio` directly

This keeps domain code portable while still making orchestration and transport non-blocking.

---

## Execution Model

The execution rule is simple:

- Rust owns execution
- TypeScript owns reach

Concretely:

1. `anima-core` runs the agent task loop
2. `anima-swarm` coordinates multiple agents and strategies
3. `anima-daemon` exposes engine capabilities over HTTP/SSE
4. TypeScript consumers call the daemon instead of embedding engine logic

This avoids splitting orchestration across languages and keeps one source of truth for runtime behavior.

---

## Migration Order

### Phase 1: Finish `anima-core` parity

Complete the single-agent runtime in Rust:

- async model/tool/provider/evaluator boundaries
- provider-backed model adapters
- parity for task loop semantics
- parity for event emission
- parity for token accounting

Exit criteria:

- Rust single-agent runs match current TS behavior for agreed deterministic scenarios

### Phase 2: Port the swarm coordinator

Port the logic from `packages/swarm/src/coordinator.ts` into `anima-swarm`.

Bring over:

- `start()`
- `dispatch()`
- `stop()`
- worker pool lifecycle
- serial dispatch chain
- per-task reset behavior
- spawn/send/broadcast hooks
- token usage aggregation
- strategy dispatch shell

Do not start with every strategy. Port the coordinator shell first, then move strategies one by one.

Exit criteria:

- the Rust coordinator can run the first strategy end to end with persistent workers

### Phase 3: Rebuild the daemon as the async boundary

Replace the blocking daemon transport with:

- `tokio`
- `axum`
- HTTP endpoints
- SSE event streams

Daemon responsibilities in this phase:

- agent registry
- swarm registry
- request validation
- event streaming
- cancellation and timeout plumbing

Exit criteria:

- the daemon can host both single-agent and swarm execution through stable async APIs

### Phase 4: Migrate TypeScript to thin clients

Move SDK and CLI execution over to daemon clients:

- HTTP control-plane calls
- SSE event consumption
- config builders remain in TS
- plugin/tool authoring remains in TS

The TypeScript engine remains available during transition as a reference implementation and fallback path.

Exit criteria:

- SDK and CLI can drive Rust execution without embedding TS runtime logic

### Phase 5: Remove embedded TS execution paths

Only once Rust parity and daemon stability are proven:

- stop defaulting to embedded TS runtime execution
- keep only compatibility shims where necessary
- remove duplicate execution logic last

Exit criteria:

- Rust is the default execution engine
- TypeScript remains a client and extension surface

---

## Cutover Criteria

Do not cut TypeScript clients over to the daemon until the Rust side has:

- stable agent APIs
- stable swarm APIs
- SSE event streaming
- cancellation and timeout behavior
- persistent worker lifecycle
- parity coverage for deterministic runtime and coordinator scenarios

The first production cutover should be opt-in. TypeScript consumers should be able to target either:

- embedded TS execution
- Rust daemon execution

Only after soak testing should the daemon become the default path.

---

## Parity Requirements

Rust must be compared against the existing TypeScript implementation for:

- single-agent runs
- tool loop behavior
- provider/evaluator behavior
- event sequencing
- token accounting
- coordinator lifecycle
- strategy behavior where deterministic comparison is realistic

Parity should be measured with fixtures and scenario tests, not assumed.

---

## Non-Goals

This design does not imply:

- rewriting the entire ecosystem in Rust
- replacing TypeScript plugin or SDK authoring
- moving HTTP concerns into `anima-core`
- keeping swarm orchestration half in TS and half in Rust
- cutting over clients before the daemon is stable

---

## Success Criteria

This design succeeds when:

- Rust owns execution across runtime and swarm
- the daemon is the only runtime boundary
- TypeScript remains the ecosystem and client surface
- `tokio` and `axum` support the async daemon/swarm layers
- cutover happens only after parity and stability are demonstrated

---

## Relationship To Existing Specs

This spec extends, but does not replace:

- `2026-04-01-animaos-rs-engine-design.md`
- `2026-04-01-animaos-rs-agent-runtime-daemon-design.md`

The earlier docs define the Rust workspace and early runtime slices. This spec defines the final ownership boundary and migration direction for runtime, swarm, daemon, and TypeScript clients.
