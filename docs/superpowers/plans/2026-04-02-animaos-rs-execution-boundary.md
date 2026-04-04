# animaOS Rust Execution Boundary Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move agent runtime and swarm execution into Rust, rebuild the daemon on `tokio`/`axum`, and leave TypeScript as a thin client and ecosystem surface.

**Architecture:** `anima-core` becomes the async-aware, transport-agnostic engine crate; `anima-swarm` becomes the `tokio` orchestration layer; `anima-daemon` becomes the `axum` HTTP/SSE boundary; TypeScript SDK and CLI become daemon clients rather than embedded runtimes. Implement this in order so each phase yields working, testable software and later phases do not start before earlier phases are green.

**Tech Stack:** Rust 2021, Cargo workspace, `async-trait`, `tokio`, `axum`, SSE, Rust unit/integration tests, existing TypeScript runtime/swarm packages, Nx-managed TS verification.

## Status Update (2026-04-03)

This document is now partly historical. The repository already implements the major Rust execution-boundary milestones that were originally planned here.

Implemented in the repo today:

- `packages/animaos-rs` passes `cargo test --manifest-path packages/animaos-rs/Cargo.toml`
- `anima-core` is already async-aware and covers runtime/tool/provider/evaluator flows
- `anima-swarm` already includes the coordinator shell, message bus, and strategy implementations
- `anima-daemon` already runs on `tokio`/`axum` and exposes agent, memory, swarm, and SSE APIs
- `packages/sdk` is already a thin daemon client surface
- CLI `run`, `chat`, `agents`, and `launch` are daemon-backed by default on this branch
- `launch` is now daemon-only; the embedded TypeScript fallback has been removed
- `launch` TUI is driven through a daemon SSE-to-event-bus bridge

Current remaining work is narrower than the task list below suggests:

- soak-test the daemon path as the only launch runtime
- keep parity checks focused on deterministic scenarios
- keep this document aligned with the actual repo state

Treat the detailed tasks below as implementation history and reference material, not as a literal current backlog.

---

## Scope Check

This spec spans four dependent subsystems:

1. async `anima-core`
2. `tokio`-based `anima-swarm`
3. `axum`-based `anima-daemon`
4. TypeScript thin clients

Keep them in one ordered plan because each phase depends on the prior one. Do not execute later tasks early. The checkpoints are:

- async single-agent parity
- swarm coordinator parity
- daemon async boundary parity
- TypeScript client cutover

Implementation discipline:

- Use `@superpowers:test-driven-development` for each task.
- Use `@superpowers:verification-before-completion` before claiming any phase is done.
- Prefer small commits at the end of each task.

---

## File Structure

### Core Async Runtime

- **Modify:** `packages/animaos-rs/crates/anima-core/Cargo.toml` - add async-trait support and any minimal async helper dependencies
- **Modify:** `packages/animaos-rs/crates/anima-core/src/components.rs` - convert provider/evaluator contracts to async-aware traits
- **Modify:** `packages/animaos-rs/crates/anima-core/src/model.rs` - convert model adapter contract to async-aware generation
- **Modify:** `packages/animaos-rs/crates/anima-core/src/runtime.rs` - convert task loop, tool loop, provider/evaluator execution, and event flow to async methods
- **Modify:** `packages/animaos-rs/crates/anima-core/src/lib.rs` - export updated async-aware contracts
- **Test:** `packages/animaos-rs/crates/anima-core/src/runtime.rs` and/or `packages/animaos-rs/crates/anima-core/tests/runtime_async.rs` - targeted async runtime parity tests

### Swarm Coordinator

- **Modify:** `packages/animaos-rs/crates/anima-swarm/Cargo.toml` - add `tokio` and any focused support crates
- **Create:** `packages/animaos-rs/crates/anima-swarm/src/types.rs` - Rust equivalents of `packages/swarm/src/types.ts`
- **Create:** `packages/animaos-rs/crates/anima-swarm/src/message_bus.rs` - inboxes, broadcast, clear/reset behavior
- **Create:** `packages/animaos-rs/crates/anima-swarm/src/coordinator.rs` - `start`, `dispatch`, `stop`, worker pool, serial task chain
- **Create:** `packages/animaos-rs/crates/anima-swarm/src/strategies/mod.rs` - strategy registry
- **Create:** `packages/animaos-rs/crates/anima-swarm/src/strategies/supervisor.rs` - first parity strategy
- **Create:** `packages/animaos-rs/crates/anima-swarm/src/strategies/round_robin.rs` - round-robin strategy parity
- **Create:** `packages/animaos-rs/crates/anima-swarm/src/strategies/dynamic.rs` - dynamic strategy parity
- **Modify:** `packages/animaos-rs/crates/anima-swarm/src/lib.rs` - export coordinator, message bus, types, strategies
- **Test:** `packages/animaos-rs/crates/anima-swarm/tests/coordinator.rs` - coordinator lifecycle and pooling tests
- **Test:** `packages/animaos-rs/crates/anima-swarm/tests/message_bus.rs` - message routing and reset tests
- **Reference:** `packages/swarm/src/coordinator.ts`
- **Reference:** `packages/swarm/src/coordinator.spec.ts`
- **Reference:** `packages/swarm/src/message-bus.ts`
- **Reference:** `packages/swarm/src/strategies/*.ts`

### Async Daemon

- **Modify:** `packages/animaos-rs/crates/anima-daemon/Cargo.toml` - add `tokio`, `axum`, and focused serialization/streaming dependencies
- **Create:** `packages/animaos-rs/crates/anima-daemon/src/app.rs` - `axum` router assembly and shared state wiring
- **Create:** `packages/animaos-rs/crates/anima-daemon/src/events.rs` - SSE event fanout and subscription helpers
- **Create:** `packages/animaos-rs/crates/anima-daemon/src/routes/swarms.rs` - swarm create/get/run endpoints
- **Modify:** `packages/animaos-rs/crates/anima-daemon/src/routes/agents.rs` - adapt existing agent routes to async handlers
- **Modify:** `packages/animaos-rs/crates/anima-daemon/src/routes/memories.rs` - adapt memory routes to async handlers
- **Modify:** `packages/animaos-rs/crates/anima-daemon/src/routes/mod.rs` - router/module wiring
- **Modify:** `packages/animaos-rs/crates/anima-daemon/src/state.rs` - async-safe registries for agent runtimes, swarm coordinators, and shared memory
- **Modify:** `packages/animaos-rs/crates/anima-daemon/src/model.rs` - swap deterministic test adapter behind the new async adapter interface
- **Modify:** `packages/animaos-rs/crates/anima-daemon/src/main.rs` - daemon bootstrap on `tokio`
- **Modify:** `packages/animaos-rs/crates/anima-daemon/src/lib.rs` - remove blocking socket server entrypoints in favor of app/server exports
- **Test:** `packages/animaos-rs/crates/anima-daemon/tests/agent_api.rs` - async agent API parity
- **Test:** `packages/animaos-rs/crates/anima-daemon/tests/memory_api.rs` - async memory API parity
- **Create:** `packages/animaos-rs/crates/anima-daemon/tests/swarm_api.rs` - async swarm and SSE integration tests

### TypeScript Thin Clients

- **Create:** `packages/sdk/src/client.ts` - shared daemon HTTP/SSE client
- **Create:** `packages/sdk/src/agents.ts` - agent client API wrappers
- **Create:** `packages/sdk/src/swarms.ts` - swarm client API wrappers
- **Modify:** `packages/sdk/src/index.ts` - export daemon client surface instead of embedded runtime defaults
- **Create:** `packages/cli/src/client.ts` - CLI daemon client wrapper
- **Modify:** `packages/cli/src/commands/run.ts` - call daemon run/sse flow
- **Modify:** `packages/cli/src/commands/chat.ts` - call daemon single-agent flow
- **Modify:** `packages/cli/src/commands/agents.ts` - call daemon list/get commands
- **Modify:** `packages/cli/src/commands/launch.ts` - wire daemon startup/config if needed
- **Test:** `packages/swarm/src/coordinator.spec.ts` - keep TS coordinator spec as reference until Rust parity is complete
- **Test:** `packages/sdk/src/index.ts` and CLI command tests as they are introduced

### Parity + Cutover

- **Create:** `docs/superpowers/plans/parity-fixtures/` only if lightweight fixtures are needed
- **Modify:** `docs/superpowers/specs/2026-04-02-animaos-rs-execution-boundary-design.md` only if design changes during execution
- **Create:** `packages/animaos-rs/crates/anima-daemon/tests/parity.rs` if a focused daemon parity suite is easier than duplicating coverage

---

## Task 1: Convert `anima-core` Contracts to Async Boundaries

**Files:**

- Modify: `packages/animaos-rs/crates/anima-core/Cargo.toml`
- Modify: `packages/animaos-rs/crates/anima-core/src/components.rs`
- Modify: `packages/animaos-rs/crates/anima-core/src/model.rs`
- Modify: `packages/animaos-rs/crates/anima-core/src/runtime.rs`
- Modify: `packages/animaos-rs/crates/anima-core/src/lib.rs`

- [ ] **Step 1: Write failing async runtime tests**

Cover:

- async model generation
- async provider context injection
- async evaluator execution
- async tool execution round-trip

- [ ] **Step 2: Run the targeted core tests and verify they fail**

Run: `cargo test --manifest-path packages/animaos-rs/Cargo.toml -p anima-core runtime_run_ -- --nocapture`

Expected: FAIL or compile failure because traits and runtime methods are still sync.

- [ ] **Step 3: Add minimal async dependencies and convert trait signatures**

Convert:

- `ModelAdapter::generate`
- `Provider::get`
- `Evaluator::validate`
- `Evaluator::evaluate`

Use `async-trait` or the smallest equivalent approach that keeps `anima-core` transport-agnostic.

- [ ] **Step 4: Convert `AgentRuntime` run methods to async**

At minimum:

- `run`
- `run_with_tools`
- internal provider/evaluator execution
- async token/event handling at the task loop boundary

- [ ] **Step 5: Update existing test adapters to the new async shape**

Keep the current deterministic test behavior intact while changing only the execution model.

- [ ] **Step 6: Run targeted core tests**

Run: `cargo test --manifest-path packages/animaos-rs/Cargo.toml -p anima-core runtime_run_ -- --nocapture`

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add packages/animaos-rs/crates/anima-core/Cargo.toml packages/animaos-rs/crates/anima-core/src/components.rs packages/animaos-rs/crates/anima-core/src/model.rs packages/animaos-rs/crates/anima-core/src/runtime.rs packages/animaos-rs/crates/anima-core/src/lib.rs
git commit -m "feat: make Rust runtime contracts async-aware"
```

---

## Task 2: Add Real Async Model Adapter Wiring

**Files:**

- Modify: `packages/animaos-rs/crates/anima-daemon/src/model.rs`
- Modify: `packages/animaos-rs/crates/anima-daemon/src/state.rs`
- Modify: `packages/animaos-rs/crates/anima-daemon/src/main.rs`
- Test: `packages/animaos-rs/crates/anima-daemon/tests/agent_api.rs`

- [ ] **Step 1: Write a failing daemon test for async model-backed agent runs**

Cover:

- daemon can await model generation
- token usage still updates
- provider/evaluator hooks still execute through the async path

- [ ] **Step 2: Run the targeted daemon test and verify it fails**

Run: `cargo test --manifest-path packages/animaos-rs/Cargo.toml -p anima-daemon --test agent_api run_agent_returns_task_result_and_completed_runtime_state -- --nocapture`

Expected: FAIL or compile failure because daemon runtime wiring is still sync.

- [ ] **Step 3: Convert daemon runtime/model wiring to async**

Keep:

- deterministic adapter for tests
- room/message/task semantics

Do not add network-specific provider code to `anima-core`.

- [ ] **Step 4: Add one real provider-backed adapter behind feature/config boundaries**

Recommended:

- keep deterministic adapter for tests
- add one real HTTP-backed adapter only after the async contract is stable

- [ ] **Step 5: Run targeted daemon agent tests**

Run: `cargo test --manifest-path packages/animaos-rs/Cargo.toml -p anima-daemon --test agent_api -- --nocapture`

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add packages/animaos-rs/crates/anima-daemon/src/model.rs packages/animaos-rs/crates/anima-daemon/src/state.rs packages/animaos-rs/crates/anima-daemon/src/main.rs packages/animaos-rs/crates/anima-daemon/tests/agent_api.rs
git commit -m "feat: wire async model adapters through Rust daemon"
```

---

## Task 3: Port the Swarm Message Bus and Types

**Files:**

- Modify: `packages/animaos-rs/crates/anima-swarm/Cargo.toml`
- Create: `packages/animaos-rs/crates/anima-swarm/src/types.rs`
- Create: `packages/animaos-rs/crates/anima-swarm/src/message_bus.rs`
- Modify: `packages/animaos-rs/crates/anima-swarm/src/lib.rs`
- Create: `packages/animaos-rs/crates/anima-swarm/tests/message_bus.rs`

- [ ] **Step 1: Port the TS swarm type shapes into Rust tests first**

Reference:

- `packages/swarm/src/types.ts`
- `packages/swarm/src/message-bus.ts`

Cover:

- inbox registration
- direct send
- broadcast
- clear/reset semantics

- [ ] **Step 2: Run the targeted swarm message bus tests and verify they fail**

Run: `cargo test --manifest-path packages/animaos-rs/Cargo.toml -p anima-swarm --test message_bus -- --nocapture`

Expected: FAIL because the files do not exist yet.

- [ ] **Step 3: Add `tokio` and implement the message bus**

Keep the bus focused:

- no HTTP
- no daemon coupling
- no strategy logic here

- [ ] **Step 4: Export the new swarm types and bus from `lib.rs`**

- [ ] **Step 5: Run the targeted swarm message bus tests**

Run: `cargo test --manifest-path packages/animaos-rs/Cargo.toml -p anima-swarm --test message_bus -- --nocapture`

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add packages/animaos-rs/crates/anima-swarm/Cargo.toml packages/animaos-rs/crates/anima-swarm/src/types.rs packages/animaos-rs/crates/anima-swarm/src/message_bus.rs packages/animaos-rs/crates/anima-swarm/src/lib.rs packages/animaos-rs/crates/anima-swarm/tests/message_bus.rs
git commit -m "feat: add tokio message bus for Rust swarm"
```

---

## Task 4: Port the Swarm Coordinator Shell

**Files:**

- Create: `packages/animaos-rs/crates/anima-swarm/src/coordinator.rs`
- Modify: `packages/animaos-rs/crates/anima-swarm/src/lib.rs`
- Create: `packages/animaos-rs/crates/anima-swarm/tests/coordinator.rs`
- Reference: `packages/swarm/src/coordinator.ts`
- Reference: `packages/swarm/src/coordinator.spec.ts`

- [ ] **Step 1: Write failing coordinator lifecycle tests**

Cover:

- `start`
- `dispatch`
- `stop`
- worker pool reuse
- serial dispatch behavior
- token aggregation snapshot

- [ ] **Step 2: Run the targeted coordinator tests and verify they fail**

Run: `cargo test --manifest-path packages/animaos-rs/Cargo.toml -p anima-swarm --test coordinator -- --nocapture`

Expected: FAIL because the coordinator is still a stub.

- [ ] **Step 3: Implement the coordinator shell with `tokio`**

Port:

- persistent worker pool
- serial dispatch chain
- reset behavior before each task
- spawn/send/broadcast hooks

- [ ] **Step 4: Keep strategies out of the coordinator shell until the lifecycle is stable**

Use a minimal strategy placeholder or one strategy hook point only.

- [ ] **Step 5: Run targeted coordinator tests**

Run: `cargo test --manifest-path packages/animaos-rs/Cargo.toml -p anima-swarm --test coordinator -- --nocapture`

Expected: PASS for lifecycle and pooling behavior.

- [ ] **Step 6: Commit**

```bash
git add packages/animaos-rs/crates/anima-swarm/src/coordinator.rs packages/animaos-rs/crates/anima-swarm/src/lib.rs packages/animaos-rs/crates/anima-swarm/tests/coordinator.rs
git commit -m "feat: port Rust swarm coordinator shell"
```

---

## Task 5: Port Swarm Strategies One by One

**Files:**

- Create: `packages/animaos-rs/crates/anima-swarm/src/strategies/mod.rs`
- Create: `packages/animaos-rs/crates/anima-swarm/src/strategies/supervisor.rs`
- Create: `packages/animaos-rs/crates/anima-swarm/src/strategies/round_robin.rs`
- Create: `packages/animaos-rs/crates/anima-swarm/src/strategies/dynamic.rs`
- Modify: `packages/animaos-rs/crates/anima-swarm/src/coordinator.rs`
- Modify: `packages/animaos-rs/crates/anima-swarm/tests/coordinator.rs`

- [ ] **Step 1: Write the failing supervisor strategy test**

Port the most deterministic case from `packages/swarm/src/coordinator.spec.ts`.

- [ ] **Step 2: Implement the supervisor strategy only**

Do not start all three strategies at once.

- [ ] **Step 3: Run targeted swarm tests**

Run: `cargo test --manifest-path packages/animaos-rs/Cargo.toml -p anima-swarm --test coordinator supervisor -- --nocapture`

Expected: PASS.

- [ ] **Step 4: Repeat for `round-robin`**

Add the failing test first, then the minimal strategy implementation.

- [ ] **Step 5: Repeat for `dynamic`**

Add the failing test first, then the minimal strategy implementation.

- [ ] **Step 6: Run the full swarm crate tests**

Run: `cargo test --manifest-path packages/animaos-rs/Cargo.toml -p anima-swarm`

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add packages/animaos-rs/crates/anima-swarm/src/strategies packages/animaos-rs/crates/anima-swarm/src/coordinator.rs packages/animaos-rs/crates/anima-swarm/tests/coordinator.rs
git commit -m "feat: port Rust swarm strategies"
```

---

## Task 6: Replace the Blocking Daemon With `axum`

**Files:**

- Modify: `packages/animaos-rs/crates/anima-daemon/Cargo.toml`
- Create: `packages/animaos-rs/crates/anima-daemon/src/app.rs`
- Create: `packages/animaos-rs/crates/anima-daemon/src/events.rs`
- Modify: `packages/animaos-rs/crates/anima-daemon/src/lib.rs`
- Modify: `packages/animaos-rs/crates/anima-daemon/src/main.rs`
- Modify: `packages/animaos-rs/crates/anima-daemon/src/routes/mod.rs`
- Modify: `packages/animaos-rs/crates/anima-daemon/tests/agent_api.rs`
- Modify: `packages/animaos-rs/crates/anima-daemon/tests/memory_api.rs`

- [ ] **Step 1: Write failing async daemon smoke tests**

Cover:

- `GET /health`
- existing agent routes
- existing memory routes

- [ ] **Step 2: Run targeted daemon tests and verify they fail under the new app entrypoint**

Run: `cargo test --manifest-path packages/animaos-rs/Cargo.toml -p anima-daemon --test health -- --nocapture`

Expected: FAIL once the old blocking server is removed from the test harness.

- [ ] **Step 3: Build an `axum` router with shared state**

Keep route behavior and JSON contracts stable while changing the transport layer.

- [ ] **Step 4: Add SSE event fanout primitives**

Do not wire every event consumer yet; get the broadcaster and subscription API in place first.

- [ ] **Step 5: Adapt existing integration tests to the async daemon app**

- [ ] **Step 6: Run daemon smoke tests**

Run: `cargo test --manifest-path packages/animaos-rs/Cargo.toml -p anima-daemon --test health -- --nocapture`

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add packages/animaos-rs/crates/anima-daemon/Cargo.toml packages/animaos-rs/crates/anima-daemon/src/app.rs packages/animaos-rs/crates/anima-daemon/src/events.rs packages/animaos-rs/crates/anima-daemon/src/lib.rs packages/animaos-rs/crates/anima-daemon/src/main.rs packages/animaos-rs/crates/anima-daemon/src/routes/mod.rs packages/animaos-rs/crates/anima-daemon/tests/health.rs packages/animaos-rs/crates/anima-daemon/tests/agent_api.rs packages/animaos-rs/crates/anima-daemon/tests/memory_api.rs
git commit -m "feat: rebuild Rust daemon on axum"
```

---

## Task 7: Expose Swarm APIs and SSE From the Daemon

**Files:**

- Create: `packages/animaos-rs/crates/anima-daemon/src/routes/swarms.rs`
- Modify: `packages/animaos-rs/crates/anima-daemon/src/state.rs`
- Modify: `packages/animaos-rs/crates/anima-daemon/src/routes/mod.rs`
- Create: `packages/animaos-rs/crates/anima-daemon/tests/swarm_api.rs`

- [ ] **Step 1: Write failing swarm daemon integration tests**

Cover:

- create swarm
- run swarm
- get swarm state
- subscribe to SSE events

- [ ] **Step 2: Run the targeted swarm API tests and verify they fail**

Run: `cargo test --manifest-path packages/animaos-rs/Cargo.toml -p anima-daemon --test swarm_api -- --nocapture`

Expected: FAIL because swarm routes do not exist yet.

- [ ] **Step 3: Add swarm registries to daemon state**

Store:

- active swarm coordinators
- event streams
- result snapshots

- [ ] **Step 4: Implement `POST /api/swarms`, `POST /api/swarms/:id/run`, `GET /api/swarms/:id`, and SSE stream route**

Keep the daemon as wiring only. Business logic stays in `anima-swarm`.

- [ ] **Step 5: Run targeted swarm API tests**

Run: `cargo test --manifest-path packages/animaos-rs/Cargo.toml -p anima-daemon --test swarm_api -- --nocapture`

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add packages/animaos-rs/crates/anima-daemon/src/routes/swarms.rs packages/animaos-rs/crates/anima-daemon/src/state.rs packages/animaos-rs/crates/anima-daemon/src/routes/mod.rs packages/animaos-rs/crates/anima-daemon/tests/swarm_api.rs
git commit -m "feat: expose Rust swarm over daemon APIs"
```

---

## Task 8: Replace SDK Runtime Exports With Daemon Clients

**Files:**

- Create: `packages/sdk/src/client.ts`
- Create: `packages/sdk/src/agents.ts`
- Create: `packages/sdk/src/swarms.ts`
- Modify: `packages/sdk/src/index.ts`
- Reference: `packages/sdk/package.json`

- [ ] **Step 1: Write failing SDK tests or fixtures for daemon client calls**

At minimum cover:

- create/run agent
- create/run swarm
- subscribe to events

- [ ] **Step 2: Run the targeted SDK test and verify it fails**

Run: `pnpm nx test @animaOS-SWARM/sdk`

Expected: FAIL because the daemon client surface does not exist yet.

- [ ] **Step 3: Implement a shared daemon HTTP/SSE client**

Keep:

- config builders in TS
- no embedded runtime logic

- [ ] **Step 4: Replace top-level runtime/coordinator exports with daemon client exports**

Preserve backward compatibility only where it is clearly necessary for migration.

- [ ] **Step 5: Run SDK tests**

Run: `pnpm nx test @animaOS-SWARM/sdk`

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add packages/sdk/src/client.ts packages/sdk/src/agents.ts packages/sdk/src/swarms.ts packages/sdk/src/index.ts
git commit -m "feat: switch SDK to Rust daemon clients"
```

---

## Task 9: Move CLI Commands to the Daemon Client

**Files:**

- Create: `packages/cli/src/client.ts`
- Modify: `packages/cli/src/index.ts`
- Modify: `packages/cli/src/commands/run.ts`
- Modify: `packages/cli/src/commands/chat.ts`
- Modify: `packages/cli/src/commands/agents.ts`
- Modify: `packages/cli/src/commands/launch.ts`

- [ ] **Step 1: Write failing CLI command tests or focused command harness checks**

Cover:

- run command uses daemon-backed execution
- chat uses single-agent daemon flow
- agents command lists daemon-backed agents

- [ ] **Step 2: Run the targeted CLI verification and verify it fails**

Run: `pnpm nx test @animaOS-SWARM/cli`

Expected: FAIL because commands still depend on embedded packages.

- [ ] **Step 3: Add a thin CLI daemon client wrapper**

Keep the CLI focused on:

- argument parsing
- presentation
- daemon transport

- [ ] **Step 4: Cut each command over one by one**

Do:

- `run`
- `chat`
- `agents`

Leave unrelated commands alone until they need daemon access.

- [ ] **Step 5: Run CLI tests**

Run: `pnpm nx test @animaOS-SWARM/cli`

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add packages/cli/src/client.ts packages/cli/src/index.ts packages/cli/src/commands/run.ts packages/cli/src/commands/chat.ts packages/cli/src/commands/agents.ts packages/cli/src/commands/launch.ts
git commit -m "feat: move CLI execution to Rust daemon"
```

---

## Task 10: Add Parity Gates and Cutover Flags

**Files:**

- Modify: `packages/animaos-rs/crates/anima-daemon/tests/agent_api.rs`
- Modify: `packages/animaos-rs/crates/anima-daemon/tests/swarm_api.rs`
- Modify: `packages/swarm/src/coordinator.spec.ts`
- Modify: `packages/sdk/src/index.ts` or client config file chosen in Task 8
- Modify: `packages/cli/src/client.ts` or config-loading path chosen in Task 9

- [ ] **Step 1: Add parity fixtures for agreed deterministic scenarios**

Compare:

- agent run behavior
- tool loop
- provider/evaluator hooks
- swarm coordinator lifecycle
- strategy outputs where realistic

- [ ] **Step 2: Add an opt-in cutover flag**

TypeScript clients must be able to target either:

- embedded TS engine
- Rust daemon

until soak testing is complete.

- [ ] **Step 3: Run Rust workspace verification**

Run: `cargo test --manifest-path packages/animaos-rs/Cargo.toml`

Expected: PASS.

- [ ] **Step 4: Run TypeScript workspace verification**

Run:

- `pnpm nx test @animaOS-SWARM/swarm`
- `pnpm nx test @animaOS-SWARM/sdk`
- `pnpm nx test @animaOS-SWARM/cli`

Expected: PASS.

- [ ] **Step 5: Format and verify**

Run:

- `cargo fmt --manifest-path packages/animaos-rs/Cargo.toml --all --check`
- `pnpm nx run-many -t lint --projects @animaOS-SWARM/swarm,@animaOS-SWARM/sdk,@animaOS-SWARM/cli`

Expected: SUCCESS.

- [ ] **Step 6: Commit**

```bash
git add packages/animaos-rs/crates/anima-daemon/tests/agent_api.rs packages/animaos-rs/crates/anima-daemon/tests/swarm_api.rs packages/swarm/src/coordinator.spec.ts packages/sdk/src/index.ts packages/cli/src/client.ts
git commit -m "feat: add parity gates for Rust execution cutover"
```

---

## Final Verification Gate

- [ ] **Step 1: Run full Rust verification**

Run: `cargo test --manifest-path packages/animaos-rs/Cargo.toml`

- [ ] **Step 2: Run focused TypeScript verification**

Run:

- `pnpm nx test @animaOS-SWARM/swarm`
- `pnpm nx test @animaOS-SWARM/sdk`
- `pnpm nx test @animaOS-SWARM/cli`

- [ ] **Step 3: Run format/lint verification**

Run:

- `cargo fmt --manifest-path packages/animaos-rs/Cargo.toml --all --check`
- `pnpm nx run-many -t lint --projects @animaOS-SWARM/swarm,@animaOS-SWARM/sdk,@animaOS-SWARM/cli`

- [ ] **Step 4: Document known parity gaps before cutover**

If anything still differs from TS:

- record it explicitly
- keep the cutover flag opt-in
- do not remove embedded TS execution yet
