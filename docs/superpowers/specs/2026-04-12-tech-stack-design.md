# AnimaOS SWARM — Tech Stack Design

**Date:** 2026-04-12
**Status:** Locked

---

## Decision Summary

AnimaOS SWARM is a **concurrent, durable, replayable AI execution engine**. It runs 50-100 agents in parallel — concurrency is a given. The hard architectural problem is not concurrency (Rust/Tokio handles that trivially) but **durable execution**: keeping agents alive for days, recovering from crashes, and maintaining a replayable history of every decision.

Core principle:
> **Database = truth. Process = executor.**

This enables replayability, auditability, deterministic recovery, and time-travel debugging — all critical for long-running AI agents.

---

## Final Stack

```
Rust (anima-daemon)     →  execution core
Postgres                →  step log, source of truth
TypeScript (apps/server)→  orchestration, API, scheduler
Redis                   →  BullMQ job queue
TypeScript (TUI/SDK/CLI)→  client tooling
React + Vite (apps/ui)  →  web dashboard
```

---

## Layer Breakdown

### 1. Rust — `packages/animaos-rs`

**Job:** Execute agents. Nothing else.

- Agent loop (think → act → tool → respond → repeat)
- Tool execution with idempotency checks
- LLM calls (all providers)
- Memory / BM25 search
- EventBus (ordered per agent)
- Writes step log to Postgres at boundaries
- Streams output via SSE

**Why Rust:** Execution core demands performance and memory safety. The agent loop and tool execution are CPU/IO intensive. Single binary, minimal footprint for cloud deployment.

**Why NOT Rust for orchestration:** Supervision trees, job scheduling, and lifecycle management require significant manual engineering in Rust. That time is better spent on execution quality.

---

### 2. Postgres — Step Log (Source of Truth)

**Job:** Be the truth.

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
  UNIQUE (agent_id, step_index)
);
```

**Key rules:**
- Checkpoint at deterministic, replayable points
- Before side effects: write `pending`
- After side effects: write `done` + idempotency key
- On restart: check idempotency key before re-executing
- Per-agent ordering enforced via `UNIQUE(agent_id, step_index)`

**Why Postgres over OTP in-memory:** GenServer state dies with the node. For day-long agents, the DB must be the truth. Elixir/OTP was considered and rejected — OTP solves runtime reliability, this system solves execution truth. Those are different problems.

---

### 3. TypeScript — `apps/server`

**Job:** Orchestration, API, scheduling.

- REST API (agents, swarms, search, health)
- WebSocket broadcast (real-time UI updates)
- BullMQ scheduler (agent wakeups, delayed retries, external triggers)
- Prisma + Postgres (reads/writes step log and agent state)
- Calls Rust daemon via HTTP/SSE for execution

**Why TypeScript:** Largest training data corpus for AI-assisted development. BullMQ, Prisma, and WebSocket are battle-tested. Keeps client tooling (TUI, SDK, CLI) in one language.

**BullMQ usage (constrained):**
- ✅ Schedule agent wakeups
- ✅ Delayed retries
- ✅ External event triggers
- ❌ NOT for the agent loop itself (loop lives in Rust)

---

### 4. Redis

**Job:** BullMQ backend only.

---

### 5. TypeScript — Client Tooling

| Package | Job |
|---------|-----|
| `packages/sdk` | Typed HTTP/WS client for TUI and CLI → server |
| `packages/tui` | Ink-based terminal operator surface |
| `packages/cli` | `animaos` CLI, agency scaffolding |

---

### 6. React + Vite — `apps/ui`

**Job:** Web dashboard for monitoring and controlling agents.

- Layout C: system bar + agent grid + detail panel
- Real-time via WebSocket (Phoenix Channels replaced by native WS in TS server)
- Zustand for UI state
- Cyberpunk monochrome + dark gold (`#c9a227`) design system (existing)
- Handles 50-100 concurrent agents in the grid view
- 3D space / pixel agent visualization deferred (future view)

---

## Why NOT Elixir

Elixir/Phoenix was seriously evaluated. The decision not to use it:

| Elixir strength | Our situation |
|---|---|
| Process = truth, in-memory state | We need DB = truth for replay |
| OTP supervision trees | We need deterministic step log |
| Runtime reliability | We need execution auditability |

Over time, Elixir systems naturally drift toward process-as-truth. For an AI execution engine that needs replay, audit, and time-travel debugging, this drift is dangerous. Postgres as truth is a stronger guarantee than BEAM process survival.

**Elixir would make life easier. Our design makes the system more powerful.**

If this system ever needs distributed multi-node deployment, Temporal (or similar durable execution platform) is the natural evolution — not Elixir.

---

## Data Flow

```
User / TUI / CLI
      ↓
TypeScript server (REST + WS)
      ↓
Rust anima-daemon (HTTP/SSE)
      ↓
Agent loop executes
      ↓  (checkpoint at boundaries)
Postgres step_log
      ↑
TypeScript reads for API / UI
      ↑
React dashboard (WebSocket)
```

---

## Cloud Deployment

- Rust daemon: single binary, minimal Docker image
- TypeScript server: Bun runtime container
- Postgres: managed (RDS, Supabase, Neon)
- Redis: managed (Upstash, Redis Cloud)
- React: static CDN

Horizontal scaling: stateless HTTP + external Postgres/Redis. WebSocket broadcast across multiple TS server instances requires Redis pub/sub adapter (future concern).

---

## What's Deferred

- 3D space / pixel agent visualization (future UI view)
- Multi-node WebSocket broadcast (Redis adapter)
- Auth / access control
- Temporal migration path (if multi-node execution needed)
