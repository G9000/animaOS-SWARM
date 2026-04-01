# animaOS Rust Engine Design Spec

**Date:** 2026-04-01
**Status:** Approved

---

## Goal

Build a Rust-based engine inside the current repository without deleting or rewriting the existing TypeScript system in place. The Rust work lives in one contained Cargo workspace and becomes the long-term engine foundation once it reaches parity for the runtime, swarm coordination, and memory layers.

---

## Scope

This project is a parallel rewrite inside the same repo, not a migration-in-place and not a new repository.

The existing TypeScript code remains the reference implementation during the rewrite:

- `packages/core`
- `packages/memory`
- `packages/swarm`

The first shipped Rust deliverable is the engine daemon only. Native Rust CLI and TUI are explicitly later phases.

---

## Repo Layout

Rust lives under a single Cargo workspace:

- `packages/animaos-rs/Cargo.toml`
- `packages/animaos-rs/crates/anima-core`
- `packages/animaos-rs/crates/anima-memory`
- `packages/animaos-rs/crates/anima-swarm`
- `packages/animaos-rs/crates/anima-daemon`

This keeps the rewrite contained, avoids scattering Rust peers across `packages/*`, and lets the TypeScript packages remain stable while Rust catches up.

---

## Architecture

The Rust engine owns the full runtime boundary once it is in use:

- agent configuration and domain types
- memory indexing and persistence
- agent runtime loop
- swarm coordination and strategy execution
- event emission

The daemon is the transport boundary. Consumers talk to the daemon instead of linking engine logic directly into Node. This keeps orchestration state, runtime flow, and event sequencing inside one coherent process.

---

## Transport

The daemon uses HTTP as its primary control surface from the start.

Initial transport choices:

- localhost-only HTTP API
- JSON request/response for commands
- SSE for event streaming
- no auth in the first milestone
- no distributed deployment concerns in the first milestone

Why HTTP first:

- it is a better long-term foundation than stdio for later TS server, UI, and future Rust clients
- it is easy to inspect and test
- it avoids redesigning the protocol once the daemon becomes the platform

The transport remains thin. Business logic stays in `anima-core`, `anima-memory`, and `anima-swarm`. `anima-daemon` only exposes and coordinates the engine.

---

## Crate Responsibilities

### `anima-core`

Owns stable domain definitions and shared abstractions:

- agent config
- message and content types
- task results
- tool call and tool result schemas
- token usage
- event types
- traits for models, tools, and internal services

### `anima-memory`

Owns memory behavior:

- BM25 indexing
- add, search, recent, forget, clear
- JSON file persistence
- ranking and filtering semantics matching the current TS implementation

### `anima-swarm`

Owns runtime and orchestration:

- single-agent runtime loop
- tool execution cycle
- evaluator and provider hooks
- message bus
- swarm coordinator
- strategy execution

### `anima-daemon`

Owns the runtime boundary:

- HTTP server
- SSE event stream
- engine lifecycle
- request validation at the API edge
- mapping transport requests to crate APIs

---

## Roadmap

### Phase 0: Foundation

Create the Cargo workspace in `packages/animaos-rs`, add the four crates, set up tracing/logging, shared test utilities, and a minimal daemon with a health endpoint.

Exit criteria:

- workspace builds cleanly
- test harness runs
- daemon starts and responds to `/health`

### Phase 1: Core Types And Memory

Port the domain model into `anima-core` and the memory engine into `anima-memory`.

Keep memory persistence simple and compatible:

- JSON file storage first
- no database work
- no storage abstraction expansion unless required by the port

Exit criteria:

- Rust types serialize and deserialize cleanly for agreed fixture payloads
- memory add/search/recent/forget/clear/save/load behavior is covered by tests

### Phase 2: Single-Agent Runtime

Port the runtime behavior from the current TS engine into `anima-swarm`:

- prompt assembly
- tool-call loop
- token accounting
- event emission
- mocked model provider support for deterministic tests

Exit criteria:

- a single Rust agent can execute end-to-end with mocked model responses
- tool calls, stop reasons, and emitted events are covered by tests

### Phase 3: Swarm Coordinator

Port the swarm lifecycle and message flow into `anima-swarm`.

Implementation order:

- worker pool lifecycle
- message bus semantics
- one strategy first
- remaining strategies after the baseline flow is stable

Exit criteria:

- one strategy runs end-to-end with pooled agents
- swarm state and result aggregation are covered by tests
- remaining strategies have a defined parity backlog if not yet implemented

### Phase 4: Daemon API

Expose the engine through HTTP plus SSE.

Initial endpoints:

- `GET /health`
- `POST /agents`
- `POST /agents/{id}/run`
- `GET /agents/{id}`
- `POST /swarms`
- `POST /swarms/{id}/run`
- `GET /swarms/{id}`
- `GET /events`
- memory search/read endpoints as needed for engine verification

Exit criteria:

- external clients can create and run agents and swarms through HTTP
- SSE streams lifecycle and tool events
- daemon can be driven without linking Rust code directly

### Phase 5: Parity Verification

Compare the Rust engine against the current TS engine using fixtures and scenario tests.

Compare at minimum:

- memory behavior
- runtime lifecycle
- event sequence shape
- token accounting shape
- coordinator lifecycle
- strategy output behavior where deterministic comparison is realistic

Exit criteria:

- documented parity bar is met for the engine daemon scope
- known gaps are explicit and prioritized

### Phase 6: Adoption Decision

Once the daemon is credible, choose the next consumer:

- native Rust CLI
- existing TS server integration
- native Rust TUI later

This phase is a decision gate, not an implementation commitment.

Exit criteria:

- the team agrees on the first post-daemon consumer
- the TS engine can remain frozen as a reference while adoption begins

---

## Non-Goals For This Project

- rewriting the current web UI in this phase
- rewriting the current Ink TUI in this phase
- deleting the TypeScript engine during the rewrite
- inventing a database-backed memory layer before parity
- full provider feature parity on day one
- scattering Rust code across multiple top-level package roots

---

## Success Criteria

This project succeeds when:

- the repo contains a stable Rust engine workspace under `packages/animaos-rs`
- the Rust daemon owns `memory + core + swarm`
- the TypeScript engine remains intact during the rewrite
- parity is measured against the TS implementation instead of assumed
- the daemon is strong enough to become the future platform for CLI, TUI, or TS integration
