# Swarm v1 Durability Checklist

> **For agentic workers:** Use this checklist when changing swarm execution, daemon runtime ownership, step persistence, retry semantics, or release-readiness docs. Keep the v1 boundary honest: durable enough means registered swarms restore after restart and retried completed tool work can be reused; it does not mean mid-turn process-crash resume for in-flight swarms.

**Goal:** Make hosted swarms reliable enough for v1 without overclaiming full crash-safe orchestration.

**V1 durability definition:** A swarm run in the Rust daemon can coordinate manager and worker agents, stream live events, persist swarm messages/memory relationships, write tool steps through the host database adapter, restore registered swarms and latest snapshots from a host-owned snapshot store, and reuse completed tool steps when the client retries the same swarm task with an explicit `retryKey` or `idempotencyKey`.

**Non-goal for v1:** Resuming an in-flight swarm from the middle of a model turn after daemon process restart. Restored running snapshots are marked failed/interrupted so callers can retry intentionally.

---

## Launch Checklist

- [x] `anima-swarm` remains host-agnostic and does not depend on HTTP, Postgres, axum, or daemon-specific runtime code.
- [x] The Rust daemon owns runnable swarm hosts and keeps engine/runtime crates under `packages/core-rust`.
- [x] Swarm create, list, get, run, and SSE event routes are covered by daemon tests.
- [x] Swarm manager and worker runtimes reset volatile task context between runs.
- [x] Swarm HTTP run requests preserve `Content.metadata` from `TaskRequest` instead of dropping it at the route boundary.
- [x] Built-in swarm strategies scope explicit retry metadata into manager, worker, delegation, batch delegation, and dynamic speaker calls.
- [x] Swarm child runtimes use fresh runtime IDs for volatile memory/provider context and stable persistence IDs for step-log lookup.
- [x] `anima-core` can separate runtime identity from persistence identity through `set_persistence_agent_id`.
- [x] Agent tool-step recovery remains explicit: no `retryKey` or `idempotencyKey`, no completed-step reuse.
- [x] Postgres and in-memory step adapters upsert by logical `(agent_id, idempotency_key)`.
- [x] Focused core test proves swarm retry metadata reaches manager and worker runs with scoped keys.
- [x] Daemon validation passes with `CI=1 CARGO_TARGET_DIR=target/validation-rust-daemon bun x nx run rust-daemon:test --skipNxCache | cat`.

---

## Required Client Contract

Clients that want retry-safe swarm runs must send one stable key per logical run:

```json
{
  "text": "Coordinate the patch",
  "metadata": {
    "retryKey": "swarm-run-2026-05-05-001"
  }
}
```

Accepted aliases are `retryKey`, `retry_key`, `idempotencyKey`, and `idempotency_key`.

The coordinator derives child keys from the root key, strategy scope, turn/delegation identity, and agent name. The agent runtime then derives deterministic tool-step keys from the scoped retry key plus the tool payload.

---

## V1.1 Follow-up

- [x] Persist registered swarm configs and latest state snapshots outside the in-memory daemon map through `ANIMAOS_RS_CONTROL_PLANE_FILE` or Postgres `host_snapshots`.
- [x] Restore registered `SwarmCoordinator` instances from host-owned snapshots at startup.
- [x] Add integration coverage that constructs a second daemon app from the same control-plane file and verifies restored agent/swarm IDs can run.
- [ ] Persist enough live message bus state to restore in-flight inboxes and participants after restart.
- [ ] Store resumable in-flight dispatch records with status, strategy, current turn/delegation state, and active agent names.
- [ ] Decide the retention policy for step logs, swarm snapshots, and message relationships.

---

## Verification Notes

Current v1 verification covers process-local retry durability plus host startup recovery from a reused control-plane file. It still does not cover killing an external daemon process in the middle of a live model turn and resuming that turn from persisted strategy state.

Run these after swarm durability changes:

```bash
bun x nx run core-rust:test --skipNxCache
CI=1 CARGO_TARGET_DIR=target/validation-rust-daemon bun x nx run rust-daemon:test --skipNxCache | cat
```