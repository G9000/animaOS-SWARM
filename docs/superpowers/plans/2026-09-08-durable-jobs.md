# Durable Jobs Implementation Plan

> **For agentic workers:** Use superpowers:subagent-driven-development for bounded ownership and independent review. User approved continuing the workplace build; no further design gate is needed.

**Goal:** Queue, persist, execute, inspect, and explicitly recover agent jobs through daemon, SDK, and web.

**Architecture:** A daemon-owned bounded job service invokes the existing AgentRunCoordinator and persists records in the existing control-plane transaction boundary. Keep the portable execution engine untouched; its deeper checkpoint bridge remains future work.

**Tech Stack:** Existing Rust/Axum/Tokio/serde persistence, TypeScript SDK, React, Nx/Bun.

- [x] Implement `hosts/rust-daemon/src/jobs.rs` (split tests/worker if needed): validated records, idempotent queue, durable claims, bounded worker, completion/reconciliation, revision actions. Test regressions before implementation.
- [x] Wire records into `state.rs` and `control_plane_store.rs` with backward-compatible defaults and rollback semantics. Wire service start/shutdown into `app.rs` and owner-authorized routes in `routes/jobs.rs`/`routes/mod.rs`.
- [x] Add SDK jobs contracts/methods and tests. Add agent-scoped Runs UI in Work hub with queue, status, retry acknowledgement, queued cancellation, and honest errors/offline states.
- [x] Review race/failure paths and contract integration. Run Rust daemon test target and SDK/web relevant targets. Exercise a disposable live queued/restart scenario if practical; no paid model calls.
- [x] Update capability/handoff descriptions to distinguish resumable queued work from interrupted work requiring review. Record evidence and remaining limits.

Acceptance evidence and deferred browser/device verification are recorded in `docs/design/2026-09-08-durable-jobs-handoff.md`. This increment used deterministic disk restore/execution tests instead of a disposable live browser restart scenario. Existing capability UI still correctly reports that interrupted execution cannot automatically continue; Runs describes the saved queue behavior.
