# animaOS Rust Memory Daemon API Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Expose the Rust memory subsystem through `anima-daemon` with create, search, and recent HTTP endpoints.

**Architecture:** Keep the daemon daemon-first and stdlib-only. `anima-daemon` will own one shared in-process `MemoryManager`, parse a small HTTP/JSON surface, and map request validation onto the already-ported memory API. Integration tests will drive a live TCP server to verify state survives across multiple requests in one daemon process.

**Tech Stack:** Rust 2021, Cargo workspace, stdlib TCP/HTTP handling, Rust integration tests, existing `anima-memory` crate.

---

## File Structure

### Daemon Crate
- **Modify:** `packages/animaos-rs/crates/anima-daemon/src/lib.rs` - shared daemon state, request parsing, routing, JSON responses
- **Modify:** `packages/animaos-rs/crates/anima-daemon/tests/health.rs` - keep existing health coverage compatible with daemon changes
- **Create:** `packages/animaos-rs/crates/anima-daemon/tests/memory_api.rs` - live integration tests for memory HTTP endpoints

### Memory Crate
- **Modify:** `packages/animaos-rs/crates/anima-memory/src/memory_manager.rs` - expose any small helpers needed by the daemon for string type conversion

---

## Task 1: Write Failing Memory API Integration Tests

**Files:**
- Create: `packages/animaos-rs/crates/anima-daemon/tests/memory_api.rs`
- Modify: `packages/animaos-rs/crates/anima-daemon/src/lib.rs`

- [ ] **Step 1: Write a helper that starts the daemon and serves a fixed number of requests**

The tests need shared daemon state across multiple HTTP calls. Add a test-only helper pattern that:
- binds `127.0.0.1:0`
- spawns the daemon in a background thread
- serves `N` requests
- lets the test send multiple TCP requests against the same process

- [ ] **Step 2: Write the create-memory success test**

Test:
- `POST /api/memories`
- JSON body with required fields
- expect `HTTP/1.1 201 Created`
- expect response JSON containing the created content and type

- [ ] **Step 3: Run the single create-memory test and verify it fails**

Run: `cargo test --manifest-path packages/animaos-rs/Cargo.toml -p anima-daemon create_memory_returns_created_memory -- --nocapture`

Expected: FAIL because `/api/memories` is not implemented.

- [ ] **Step 4: Write the search-roundtrip test**

Test:
- create a memory with `POST /api/memories`
- query `GET /api/memories/search?q=...`
- expect `HTTP/1.1 200 OK`
- expect `{ "results": [...] }` containing the created memory

- [ ] **Step 5: Write the recent-memories test**

Test:
- create two memories in order
- query `GET /api/memories/recent`
- expect newest-first ordering

- [ ] **Step 6: Write validation failure tests**

Cover at least:
- missing required body field on `POST /api/memories` returns `400`
- missing `q` on `GET /api/memories/search` returns `400`

---

## Task 2: Add Minimal Daemon State and Routing

**Files:**
- Modify: `packages/animaos-rs/crates/anima-daemon/src/lib.rs`

- [ ] **Step 1: Replace placeholder daemon-owned fields with shared state**

Introduce a small internal state container with:
- one `MemoryManager`
- existing placeholder swarm field if still needed

Use a shared owner so multiple requests in the same daemon process see the same memory state.

- [ ] **Step 2: Add a `serve_n()` helper for integration tests**

Implement:

```rust
pub fn serve_n(self, limit: usize) -> std::io::Result<()>
```

Behavior:
- accept and handle exactly `limit` incoming connections
- reuse the same daemon state across all handled requests

- [ ] **Step 3: Add minimal request parsing**

Parse:
- method
- path
- query string
- JSON request body for `POST /api/memories`

Do not build a general web framework. Keep it to the exact daemon API needs.

- [ ] **Step 4: Add routing**

Handle:
- `GET /health`
- `POST /api/memories`
- `GET /api/memories/search`
- `GET /api/memories/recent`
- fallback `404`

---

## Task 3: Implement Memory Endpoint Behavior

**Files:**
- Modify: `packages/animaos-rs/crates/anima-daemon/src/lib.rs`
- Modify: `packages/animaos-rs/crates/anima-memory/src/memory_manager.rs` (only if string conversion helpers are needed)

- [ ] **Step 1: Implement create-memory validation**

Validate:
- `agentId`
- `agentName`
- `type`
- `content`
- `importance`
- optional `tags`

Return `400` JSON errors for invalid inputs.

- [ ] **Step 2: Map request payloads into `NewMemory`**

Convert JSON fields into the Rust `anima-memory` types and insert through the shared `MemoryManager`.

- [ ] **Step 3: Implement search endpoint behavior**

Parse query params:
- `q`
- `agentId`
- `agentName`
- `type`
- `limit`
- `minImportance`

Return:

```json
{ "results": [...] }
```

- [ ] **Step 4: Implement recent endpoint behavior**

Parse query params:
- `agentId`
- `agentName`
- `limit`

Return:

```json
{ "memories": [...] }
```

- [ ] **Step 5: Serialize memory values into stable JSON responses**

Keep response bodies small, explicit, and aligned with the request field naming used by the TS server (`agentId`, `agentName`, `createdAt`, etc.).

---

## Task 4: Verify and Keep Existing Behavior Green

**Files:**
- Modify: `packages/animaos-rs/crates/anima-daemon/tests/health.rs` (only if test helpers or method names change)

- [ ] **Step 1: Run the targeted daemon memory tests**

Run: `cargo test --manifest-path packages/animaos-rs/Cargo.toml -p anima-daemon memory_api -- --nocapture`

Expected: PASS.

- [ ] **Step 2: Run the existing health test**

Run: `cargo test --manifest-path packages/animaos-rs/Cargo.toml -p anima-daemon health_endpoint_returns_ok_json -- --nocapture`

Expected: PASS.

- [ ] **Step 3: Run the full Rust workspace tests**

Run: `cargo test --manifest-path packages/animaos-rs/Cargo.toml`

Expected: PASS.

- [ ] **Step 4: Format the Rust workspace**

Run: `cargo fmt --manifest-path packages/animaos-rs/Cargo.toml --all`

Expected: SUCCESS.
