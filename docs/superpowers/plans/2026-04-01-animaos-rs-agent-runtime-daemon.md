# animaOS Rust Agent Runtime Daemon Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add real in-process `AgentRuntime` instances to the Rust engine and expose them through daemon agent endpoints.

**Architecture:** `anima-core` will own a lifecycle-oriented runtime object with state, messages, last task result, and event log. `anima-daemon` will store those runtime objects in shared process state, serialize runtime snapshots over HTTP, and reuse the existing memory manager for per-agent recent memory queries.

**Tech Stack:** Rust 2021, Cargo workspace, stdlib TCP/HTTP handling, Rust unit tests, Rust integration tests, existing `anima-core` and `anima-memory` crates.

---

## File Structure

### Core Runtime
- **Create:** `packages/animaos-rs/crates/anima-core/src/runtime.rs` - runtime object, snapshot type, lifecycle methods, runtime tests
- **Modify:** `packages/animaos-rs/crates/anima-core/src/lib.rs` - export runtime types
- **Modify:** `packages/animaos-rs/crates/anima-core/src/agent.rs` - add small helpers needed for status serialization if required

### Daemon
- **Modify:** `packages/animaos-rs/crates/anima-daemon/src/state.rs` - runtime registry ownership
- **Create:** `packages/animaos-rs/crates/anima-daemon/src/routes/agents.rs` - agent create/list/get/recent-memory handlers
- **Modify:** `packages/animaos-rs/crates/anima-daemon/src/routes/mod.rs` - route dispatch for agent endpoints
- **Modify:** `packages/animaos-rs/crates/anima-daemon/src/json.rs` - only if recursive JSON-to-core-value conversion helpers are needed

### Tests
- **Create:** `packages/animaos-rs/crates/anima-daemon/tests/agent_api.rs` - live daemon agent endpoint tests

---

## Task 1: Write Failing Agent API Integration Tests

**Files:**
- Create: `packages/animaos-rs/crates/anima-daemon/tests/agent_api.rs`

- [ ] **Step 1: Write a daemon spawn helper for the new integration test file**

- [ ] **Step 2: Write `POST /api/agents` success test**

Expected response:
- `201 Created`
- response contains runtime `id`, `name`, `status`
- response contains `messageCount`, `eventCount`, `lastTask`

- [ ] **Step 3: Run the create-agent test and verify it fails**

Run: `cargo test --manifest-path packages/animaos-rs/Cargo.toml -p anima-daemon create_agent_returns_runtime_snapshot -- --nocapture`

Expected: FAIL because the route is not implemented.

- [ ] **Step 4: Write list and get tests**

Cover:
- `GET /api/agents` includes the created runtime
- `GET /api/agents/:id` returns the runtime snapshot
- unknown id returns `404`

- [ ] **Step 5: Write per-agent recent memories test**

Flow:
- create agent
- create one memory matching runtime `agentId`
- create one memory for another id
- `GET /api/agents/:id/memories/recent` returns only the matching memory

- [ ] **Step 6: Write validation failure test for create-agent**

At minimum:
- missing `name` or `model` returns `400`

---

## Task 2: Add Runtime Type to `anima-core`

**Files:**
- Create: `packages/animaos-rs/crates/anima-core/src/runtime.rs`
- Modify: `packages/animaos-rs/crates/anima-core/src/lib.rs`
- Modify: `packages/animaos-rs/crates/anima-core/src/agent.rs`

- [ ] **Step 1: Write failing unit tests for runtime lifecycle and snapshot**

Cover:
- `new + init` yields idle state and one spawn event
- `record_message` increments message count
- `mark_completed` stores last task result and updates status
- `stop` sets terminated status

- [ ] **Step 2: Run the targeted runtime test and verify it fails**

Run: `cargo test --manifest-path packages/animaos-rs/Cargo.toml -p anima-core runtime_tracks_lifecycle_state -- --nocapture`

Expected: FAIL because `runtime.rs` does not exist yet.

- [ ] **Step 3: Implement minimal runtime type and snapshot**

Add:
- `AgentRuntime`
- `AgentRuntimeSnapshot`
- lifecycle methods
- accessor methods for id, state, snapshot, messages, and last task

- [ ] **Step 4: Export the runtime types**

Update `anima-core/src/lib.rs` so daemon code can use the runtime cleanly.

- [ ] **Step 5: Run the targeted runtime tests**

Expected: PASS.

---

## Task 3: Move Daemon State to Runtime Ownership

**Files:**
- Modify: `packages/animaos-rs/crates/anima-daemon/src/state.rs`

- [ ] **Step 1: Replace plain agent metadata ownership with a runtime registry**

Store:
- `HashMap<String, AgentRuntime>`
- existing `MemoryManager`

- [ ] **Step 2: Add small state helper methods**

Helpers should cover:
- create runtime
- list snapshots
- get snapshot by id
- fetch recent memories for one runtime id

- [ ] **Step 3: Keep the helpers focused**

Do not leak mutable internals through route code if a focused state method is enough.

---

## Task 4: Implement Agent Routes

**Files:**
- Create: `packages/animaos-rs/crates/anima-daemon/src/routes/agents.rs`
- Modify: `packages/animaos-rs/crates/anima-daemon/src/routes/mod.rs`

- [ ] **Step 1: Parse agent create payload**

Validate:
- `name`
- `model`
- optional strings
- optional string arrays
- optional settings object

- [ ] **Step 2: Convert JSON values into `AgentConfig`**

Preserve known settings fields:
- `temperature`
- `maxTokens`
- `timeout`
- `maxRetries`

- [ ] **Step 3: Implement `POST /api/agents`**

Create runtime through daemon state and return:

```json
{ "agent": { ...snapshot... } }
```

- [ ] **Step 4: Implement `GET /api/agents` and `GET /api/agents/:id`**

Return stable JSON snapshot shapes and `404` for unknown ids.

- [ ] **Step 5: Implement `GET /api/agents/:id/memories/recent`**

Use runtime `agentId` to filter the existing memory manager and return:

```json
{ "memories": [...] }
```

---

## Task 5: Verify the Full Rust Slice

**Files:**
- Modify as needed from previous tasks only

- [ ] **Step 1: Run targeted daemon agent tests**

Run: `cargo test --manifest-path packages/animaos-rs/Cargo.toml -p anima-daemon --test agent_api -- --nocapture`

Expected: PASS.

- [ ] **Step 2: Run targeted core runtime tests**

Run: `cargo test --manifest-path packages/animaos-rs/Cargo.toml -p anima-core runtime_ -- --nocapture`

Expected: PASS.

- [ ] **Step 3: Run the full Rust workspace tests**

Run: `cargo test --manifest-path packages/animaos-rs/Cargo.toml`

Expected: PASS.

- [ ] **Step 4: Format the Rust workspace**

Run: `cargo fmt --manifest-path packages/animaos-rs/Cargo.toml --all`

Expected: SUCCESS.
