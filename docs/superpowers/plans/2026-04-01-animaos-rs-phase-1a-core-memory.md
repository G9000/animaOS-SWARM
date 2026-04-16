# animaOS Rust Phase 1A Plan: Core + Memory Parity

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:test-driven-development while implementing this plan. Keep the scope to `anima-core` and `anima-memory`; do not start swarm/runtime parity in this phase.

**Goal:** Port the current TypeScript core type surface and memory subsystem into Rust with behavior pinned by tests. At the end of this phase, the Rust crates should preserve the current TS behavior for core data shapes, BM25 search, memory filtering, recent retrieval, forgetting, clearing, and JSON persistence.

**Architecture:** `anima-core` becomes the shared model crate for IDs, content, task results, agent config, and engine events. `anima-memory` owns a local in-process `MemoryManager` backed by an internal BM25 index. The public API should stay small and deterministic so the daemon can depend on it later without rework.

**Tech Stack:** Rust 2021, Cargo workspace, `serde`/`serde_json` for JSON persistence, `uuid` for memory IDs, `tempfile` for tests.

---

## File Targets

### Core Crate
- **Modify:** `packages/animaos-rs/crates/anima-core/Cargo.toml`
- **Modify:** `packages/animaos-rs/crates/anima-core/src/lib.rs`
- **Modify:** `packages/animaos-rs/crates/anima-core/src/agent.rs`
- **Modify:** `packages/animaos-rs/crates/anima-core/src/events.rs`
- **Modify:** `packages/animaos-rs/crates/anima-core/src/primitives.rs`

### Memory Crate
- **Modify:** `packages/animaos-rs/crates/anima-memory/Cargo.toml`
- **Modify:** `packages/animaos-rs/crates/anima-memory/src/lib.rs`
- **Create:** `packages/animaos-rs/crates/anima-memory/src/bm25.rs`

---

## Task 1: Expand `anima-core` to Match TS Data Shapes

- [ ] Add `serde` support to `anima-core` so core types can be persisted and transported later.
- [ ] Port the TypeScript primitives into Rust:
  - `AttachmentType`, `Attachment`, `Content`, `MessageRole`, `Message`
  - `TaskStatus`, `TaskResult<T>`
  - `AgentId`, `RoomId`, `MessageId`, `UuidString`
- [ ] Port agent types into Rust:
  - `AgentConfig`
  - `AgentSettings`
  - `AgentStatus`
  - `TokenUsage`
  - `AgentState`
- [ ] Port event types into Rust:
  - event type enum covering the current TS string union
  - generic event envelope with `event_type`, `agent_id`, `timestamp`, `data`

**Verification:**
- `cargo test --manifest-path packages/animaos-rs/Cargo.toml -p anima-core`

---

## Task 2: Add Failing BM25 Tests in `anima-memory`

- [ ] Write Rust unit tests that pin the existing TS BM25 behavior:
  - indexing and searching documents
  - ranking repeated terms higher
  - empty results for unmatched and blank queries
  - document removal
  - `clear()`
  - re-adding the same document ID replaces the old content
  - `limit` is respected
- [ ] Keep tokenization behavior aligned with TS:
  - lowercase normalization
  - non-alphanumeric stripping
  - stop-word filtering
  - simple suffix stemming

**Verification:**
- `cargo test --manifest-path packages/animaos-rs/Cargo.toml -p anima-memory bm25`

---

## Task 3: Add Failing `MemoryManager` Tests in `anima-memory`

- [ ] Write Rust tests for `MemoryManager::add()`:
  - unique IDs
  - timestamps set on insert
  - provided fields preserved
  - size increments
  - new memories are immediately searchable
- [ ] Write Rust tests for `search()`:
  - relevant ranking
  - positive scores
  - blank/no-match handling
  - filters for `agent_id`, `agent_name`, `memory_type`, `min_importance`, `limit`
  - combined filters
- [ ] Write Rust tests for:
  - `get_recent()`
  - `forget()`
  - `clear()`
  - `save()` / `load()`
  - `summary()`
- [ ] Preserve current TS behavior for now, including the known `"1 memories"` summary bug, so the Rust port stays parity-correct before we fix behavior intentionally.

**Verification:**
- `cargo test --manifest-path packages/animaos-rs/Cargo.toml -p anima-memory memory_manager`

---

## Task 4: Implement `BM25` and `MemoryManager`

- [ ] Implement internal `BM25` support with:
  - `add_document`
  - `remove_document`
  - `search`
  - `clear`
  - `size`
- [ ] Implement `MemoryType`, `Memory`, `MemorySearchResult`, and `MemorySearchOptions`.
- [ ] Implement `MemoryManager` with:
  - in-memory map storage
  - BM25-backed indexing of content + type + agent name + tags
  - filtering after over-fetch
  - recent sorting by `created_at` descending
  - optional JSON persistence file path
  - tolerant `load()` behavior for missing or corrupted files

**Verification:**
- `cargo test --manifest-path packages/animaos-rs/Cargo.toml -p anima-memory`

---

## Task 5: Regressions and Formatting

- [ ] Run the full Rust workspace tests.
- [ ] Run `cargo fmt --all`.
- [ ] Document any intentional gaps that remain after Phase 1A:
  - no agent runtime parity yet
  - no daemon endpoints beyond health
  - no swarm behavior port yet

**Verification:**
- `cargo test --manifest-path packages/animaos-rs/Cargo.toml`
- `cargo fmt --manifest-path packages/animaos-rs/Cargo.toml --all --check`
