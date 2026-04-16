# animaOS Rust Phase 0 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create the initial Rust workspace under `packages/animaos-rs` with four crates and a minimal daemon that starts and responds to `GET /health`.

**Architecture:** Phase 0 keeps the rewrite contained in one Cargo workspace. `anima-core`, `anima-memory`, and `anima-swarm` start as small, compilable crates with stable placeholder types, while `anima-daemon` owns the first transport boundary with a tiny localhost HTTP server implemented in Rust. The daemon depends on the internal crates but does not attempt parity with the TypeScript engine yet.

**Tech Stack:** Rust 2021, Cargo workspace, standard library networking/threading, Rust unit + integration tests.

---

## File Structure

### Workspace Root
- **Create:** `packages/animaos-rs/Cargo.toml` - workspace members, shared package metadata, shared lint/profile defaults
- **Create:** `packages/animaos-rs/README.md` - workspace purpose and crate map
- **Create:** `packages/animaos-rs/.gitignore` - ignore `target/`

### Core Crate
- **Create:** `packages/animaos-rs/crates/anima-core/Cargo.toml` - core crate manifest
- **Create:** `packages/animaos-rs/crates/anima-core/src/lib.rs` - exports public modules
- **Create:** `packages/animaos-rs/crates/anima-core/src/agent.rs` - `AgentConfig` placeholder type aligned with TS names
- **Create:** `packages/animaos-rs/crates/anima-core/src/events.rs` - `EventType` + `EngineEvent`
- **Create:** `packages/animaos-rs/crates/anima-core/src/primitives.rs` - `TaskResult`, `HealthStatus`, ids/type aliases

### Memory Crate
- **Create:** `packages/animaos-rs/crates/anima-memory/Cargo.toml` - memory crate manifest
- **Create:** `packages/animaos-rs/crates/anima-memory/src/lib.rs` - exports `MemoryManager`

### Swarm Crate
- **Create:** `packages/animaos-rs/crates/anima-swarm/Cargo.toml` - swarm crate manifest
- **Create:** `packages/animaos-rs/crates/anima-swarm/src/lib.rs` - exports `SwarmCoordinator`

### Daemon Crate
- **Create:** `packages/animaos-rs/crates/anima-daemon/Cargo.toml` - daemon crate manifest
- **Create:** `packages/animaos-rs/crates/anima-daemon/src/lib.rs` - daemon server API used by tests
- **Create:** `packages/animaos-rs/crates/anima-daemon/src/main.rs` - binary entry point
- **Create:** `packages/animaos-rs/crates/anima-daemon/tests/health.rs` - integration test for live `/health`

---

## Task 1: Scaffold the Cargo Workspace

**Files:**
- Create: `packages/animaos-rs/Cargo.toml`
- Create: `packages/animaos-rs/README.md`
- Create: `packages/animaos-rs/.gitignore`

- [ ] **Step 1: Create the workspace manifest**

Add a workspace manifest with these members:

```toml
[workspace]
members = [
  "crates/anima-core",
  "crates/anima-memory",
  "crates/anima-swarm",
  "crates/anima-daemon",
]
resolver = "2"

[workspace.package]
edition = "2021"
license = "MIT"
version = "0.1.0"
authors = ["animaOS contributors"]
```

- [ ] **Step 2: Add a small workspace README**

Document that Phase 0 only guarantees:
- crate layout exists
- workspace compiles
- daemon exposes `GET /health`

- [ ] **Step 3: Ignore Cargo build artifacts**

Add:

```gitignore
/target
```

- [ ] **Step 4: Verify Cargo sees the workspace**

Run: `cargo metadata --format-version 1 --manifest-path packages/animaos-rs/Cargo.toml`

Expected: JSON output containing all four crate manifests.

---

## Task 2: Add Compilable Placeholder Engine Crates

**Files:**
- Create: `packages/animaos-rs/crates/anima-core/Cargo.toml`
- Create: `packages/animaos-rs/crates/anima-core/src/lib.rs`
- Create: `packages/animaos-rs/crates/anima-core/src/agent.rs`
- Create: `packages/animaos-rs/crates/anima-core/src/events.rs`
- Create: `packages/animaos-rs/crates/anima-core/src/primitives.rs`
- Create: `packages/animaos-rs/crates/anima-memory/Cargo.toml`
- Create: `packages/animaos-rs/crates/anima-memory/src/lib.rs`
- Create: `packages/animaos-rs/crates/anima-swarm/Cargo.toml`
- Create: `packages/animaos-rs/crates/anima-swarm/src/lib.rs`

- [ ] **Step 1: Define minimal core types**

Create:

```rust
pub struct AgentConfig {
    pub name: String,
    pub model: String,
}

pub enum EventType {
    HealthCheck,
}

pub struct TaskResult<T> {
    pub status: &'static str,
    pub data: Option<T>,
    pub error: Option<String>,
    pub duration_ms: u128,
}
```

- [ ] **Step 2: Add a memory placeholder**

Expose a minimal memory manager:

```rust
#[derive(Default)]
pub struct MemoryManager;

impl MemoryManager {
    pub fn new() -> Self {
        Self
    }
}
```

- [ ] **Step 3: Add a swarm placeholder**

Expose a minimal coordinator:

```rust
#[derive(Default)]
pub struct SwarmCoordinator;

impl SwarmCoordinator {
    pub fn new() -> Self {
        Self
    }
}
```

- [ ] **Step 4: Verify the library crates compile**

Run: `cargo test --manifest-path packages/animaos-rs/Cargo.toml --lib`

Expected: all three library crates compile successfully.

---

## Task 3: Write the Failing Daemon Health Test

**Files:**
- Create: `packages/animaos-rs/crates/anima-daemon/Cargo.toml`
- Create: `packages/animaos-rs/crates/anima-daemon/src/lib.rs`
- Create: `packages/animaos-rs/crates/anima-daemon/src/main.rs`
- Create: `packages/animaos-rs/crates/anima-daemon/tests/health.rs`

- [ ] **Step 1: Create the daemon crate manifest**

Depend on:

```toml
anima-core = { path = "../anima-core" }
anima-memory = { path = "../anima-memory" }
anima-swarm = { path = "../anima-swarm" }
```

- [ ] **Step 2: Write the failing integration test**

Create `tests/health.rs` with a live server test that:
- binds the daemon on `127.0.0.1:0`
- starts it on a background thread
- issues `GET /health` using `TcpStream`
- expects `HTTP/1.1 200 OK`
- expects a JSON body containing `"status":"ok"`

- [ ] **Step 3: Run the health test and watch it fail**

Run: `cargo test --manifest-path packages/animaos-rs/Cargo.toml -p anima-daemon health -- --nocapture`

Expected: FAIL because the daemon server API does not exist yet.

---

## Task 4: Implement the Minimal Health-Checked Daemon

**Files:**
- Modify: `packages/animaos-rs/crates/anima-daemon/src/lib.rs`
- Modify: `packages/animaos-rs/crates/anima-daemon/src/main.rs`

- [ ] **Step 1: Add the smallest server API needed by the test**

Implement a small daemon type with:
- `bind("127.0.0.1:0")`
- `local_addr()`
- `serve_one()` or `serve_until_shutdown()` style method
- request handling for:
  - `GET /health` -> `200 OK` with `{"status":"ok"}`
  - all other routes -> `404 Not Found`

- [ ] **Step 2: Add the binary entry point**

`main.rs` should:
- read `ANIMAOS_RS_HOST` / `ANIMAOS_RS_PORT` with sane defaults
- print the bound address
- start the daemon loop

- [ ] **Step 3: Re-run the daemon test**

Run: `cargo test --manifest-path packages/animaos-rs/Cargo.toml -p anima-daemon health -- --nocapture`

Expected: PASS.

- [ ] **Step 4: Run the full workspace test suite**

Run: `cargo test --manifest-path packages/animaos-rs/Cargo.toml`

Expected: PASS for the whole Phase 0 workspace.

---

## Task 5: Smoke-Check the Binary

**Files:**
- Modify: `packages/animaos-rs/crates/anima-daemon/src/main.rs` (only if needed)

- [ ] **Step 1: Build the workspace**

Run: `cargo build --manifest-path packages/animaos-rs/Cargo.toml`

Expected: SUCCESS.

- [ ] **Step 2: Smoke-test the daemon binary**

Run the daemon manually, then confirm `/health` responds with `200 OK`.

- [ ] **Step 3: Record any follow-up gaps**

Capture any deliberate Phase 0 omissions:
- no SSE yet
- no agent/swarm endpoints yet
- placeholder core/memory/swarm crates only

These are expected and should remain Phase 1+ work.
