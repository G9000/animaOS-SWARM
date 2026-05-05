# AnimaOS SWARM — Tech Stack Design

**Date:** 2026-04-12
**Status:** Revised To Match Current Workspace

---

## Decision Summary

AnimaOS SWARM is currently a host-agnostic runtime workspace centered on reusable Rust core crates plus a single runnable Rust host. The current system is moving toward durable, replayable execution, but the live implementation today is narrower than the original April design.

Core principle:
> **Reusable core = behavior. Runnable host = execution boundary.**

Today that means:

- `packages/core-rust` owns the reusable Rust execution crates.
- `hosts/rust-daemon` is the production-ready HTTP and SSE host.
- Postgres is optional persistence for step logs and explicit retry-key reuse, not a full process-resume system.
- TypeScript packages are client and operator tooling, not the source of truth for runtime semantics.

---

## Current Stack

```
packages/core-rust         →  reusable Rust runtime crates
hosts/rust-daemon          →  runnable Axum HTTP/SSE host
Postgres (optional)        →  step-log persistence + explicit retry-key reuse
packages/sdk / cli / tui   →  TypeScript client and operator tooling
packages/core-ts           →  shared TypeScript core port for local tooling
tools/workspace-dev        →  local host selection + dev orchestration
apps/web / playground      →  browser runtime surfaces
apps/docs                  →  Astro/Starlight documentation site
apps/server                →  retained legacy local TS server, not execution truth
```

---

## Layer Breakdown

### 1. Rust Core — `packages/core-rust`

**Job:** Own reusable execution semantics without host-specific I/O.

- `anima-core` owns the agent loop, tool boundaries, lifecycle state, and persistence trait.
- `anima-memory` owns recall, retention, entities, relationships, and vector-aware memory logic.
- `anima-swarm` owns coordinator strategies, message bus, and swarm lifecycle.
- The core crates do not depend on HTTP frameworks, DB drivers, or host-specific runtimes.

**Why Rust:** The execution core is CPU and I/O heavy, benefits from strong type boundaries, and needs a portable host-agnostic library surface.

---

### 2. Rust Host — `hosts/rust-daemon`

**Job:** Provide the current runnable runtime boundary.

- Axum HTTP routes for agents, swarms, memory, health, readiness, metrics, and docs.
- SSE stream for swarm events.
- Runtime model adapter for real provider calls.
- Tool registry and shared execution context.
- Optional Postgres-backed step persistence.
- In-memory live control plane for running agents and swarms, with optional JSON snapshot recovery for registrations and latest states.

**Current constraint:** The daemon can rehydrate registered agents and swarms when `ANIMAOS_RS_CONTROL_PLANE_FILE` is configured, but it does not resume a model turn from the middle after process restart. Interrupted work is restored as failed and should be retried intentionally.

---

### 3. Optional Postgres — Step Log

**Job:** Persist tool-step boundaries and enable explicit retry reuse.

Every agent action is persisted as a step before/after execution:

```sql
CREATE TABLE step_log (
  id              UUID PRIMARY KEY,
  agent_id        TEXT NOT NULL,
  step_index      INTEGER NOT NULL,
  idempotency_key TEXT NOT NULL,
  type            TEXT NOT NULL,  -- tool:before, tool:after, task:started, etc.
  status          TEXT NOT NULL,  -- pending | done | failed
  input           JSONB,
  output          JSONB,
  created_at      TIMESTAMPTZ DEFAULT now(),
  UNIQUE (agent_id, step_index),
  UNIQUE (agent_id, idempotency_key)
);
```

**Key rules:**
- Checkpoint at deterministic, replayable points
- Before side effects: write `pending`
- After side effects: write `done` + idempotency key
- On explicit retried request with the same retry key: check idempotency key before re-executing completed tool steps
- Per-agent ordering enforced via `UNIQUE(agent_id, step_index)`
- Logical retry reuse enforced via `UNIQUE(agent_id, idempotency_key)`
- Full process restart rehydration remains follow-up work

**Important limitation:** This is not yet a full durable runtime. The current daemon can reuse completed tool steps on an explicit retry, but it cannot restore a live in-flight agent or swarm after a process restart.

---

### 4. TypeScript Tooling — `packages/sdk`, `packages/cli`, `packages/tui`, `packages/core-ts`

**Job:** Provide developer-facing and operator-facing tooling around the runnable Rust host.

- `packages/sdk` is the typed TypeScript client for the daemon.
- `packages/cli` is the local CLI surface.
- `packages/tui` is the primary local operator surface.
- `packages/core-ts` is a shared TS support layer used by local tooling. It is not the execution source of truth.

---

### 5. Workspace Orchestration — `tools/workspace-dev`

**Job:** Start the selected host and local surfaces together in development.

- `bun dev --host rust` is the normal local workflow.
- Host selection is centralized here rather than being hardcoded into UI or client packages.
- The current supported host keys are `rust`, `elixir`, and `python`, but only `rust` is production-ready.

---

### 6. Browser And Legacy App Surfaces

- `apps/web` and `apps/playground` are the current browser runtime surfaces.
- `apps/docs` is the Astro/Starlight docs site.
- `apps/server` still exists as a legacy local TypeScript server surface, but it is not the runtime source of truth and should not be confused with `hosts/rust-daemon`.

---

## Data Flow

```
User / CLI / TUI / Web / Playground
      ↓
SDK or direct HTTP client
      ↓
Rust anima-daemon (HTTP/SSE)
      ↓
packages/core-rust crates execute
      ↓  (checkpoint at boundaries)
Optional Postgres step_log
```

---

## Cloud Deployment

- Rust daemon: single binary, minimal Docker image
- Postgres: optional managed persistence for step logs
- Browser apps: static assets or preview deployments
- CLI/TUI/SDK: local or developer tooling packages

If a larger orchestration layer is introduced later, it should be documented as a new runtime boundary rather than treated as the current default architecture.

---

## What's Deferred

- Full live-runtime rehydration after daemon restart
- Multi-node execution orchestration and durable scheduling
- Additional runnable hosts beyond `hosts/rust-daemon`
- Browser-surface maturity parity with the TUI
- Any Redis/BullMQ-based scheduler or separate orchestration server, if that architecture is ever reintroduced
