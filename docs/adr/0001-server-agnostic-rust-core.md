# ADR-0001: Server-Agnostic Rust Core

**Date:** 2026-04-12
**Status:** Accepted

---

## Context

AnimaOS SWARM is being built toward durable execution — a system with replayable history, explicit step persistence, and eventually crash-safe resume.

Two problems need solving simultaneously:

**Execution truth.** Where does the authoritative record of what an agent did live? An agent may run for hours or days. If the process dies mid-execution, the system must know exactly where to resume without re-running side effects. This requires a persistent, ordered log of every agent action — not just in-process state.

**Portability.** The agent engine should not force a specific server technology on anyone. Different teams embed AI agent runtimes in different stacks — TypeScript backends, Python services, Elixir apps, serverless functions. If the core is coupled to a specific framework, every integration becomes a port.

---

## Decision

> **Core defines behavior. Host provides reality.**

`anima-core` is a **server-agnostic Rust library**. It owns the agent execution model and nothing else.

All infrastructure concerns — persistence, event emission, model provider access, scheduling semantics — are expressed as **trait interfaces** that the host implements and injects. The core never imports a database driver, an HTTP framework, or an async runtime. It calls traits.

Any host in any language can embed `anima-core` by implementing its traits. This is architecturally true. Operationally, the native Rust host is the primary path — WASM and FFI embeddings are valid but carry additional complexity that the core does not abstract away.

**The database is the source of truth. The process is the executor.** Every agent action is checkpointed through persistence traits before and after execution. The host decides what backs those traits. The core doesn't care.

**Implementation note (2026-05-05).** The current Rust daemon persists step logs and can reuse completed tool steps when a caller retries a request with the same explicit metadata retry key. It can also restore registered agents, registered swarms, and latest snapshots from `ANIMAOS_RS_CONTROL_PLANE_FILE`. Full crash-safe mid-turn resume remains future work; interrupted work is restored as failed.

**The core owns execution semantics. The host owns execution reality.** This includes timing — retry policy, wakeup schedule, and delay between steps are defined by the core as data. The host reads that data and acts on it using whatever scheduler it has. This prevents retry semantics from diverging across host implementations.

**The core is executor-agnostic.** It expresses async work through standard Rust futures, but never chooses what drives them. The native host brings tokio. A WASM host brings a browser event loop. A sync FFI host bridges with a blocking executor. The core works with any of them. No async runtime — including tokio — may be a dependency of `anima-core`.

**The core emits events through a trait.** Agent lifecycle events — tool calls, task completion, errors — flow out of the core via an `EventSink` trait. The host decides how to route them: SSE, WebSocket, logs, or nothing. This prevents transport assumptions from leaking into the core.

**Idempotency is a core rule, but current recovery is partial.** When a host provides persistence, the core can record tool-step boundaries and skip re-executing completed tool steps for an explicit retried request. Hosts can still run without persistence. The current daemon restores control-plane registrations through a host-owned JSON snapshot, but it does not resume interrupted in-flight model turns after process restart.

**A step is only considered complete when persisted as `done`.** A step written as `pending` means the side effect has not yet occurred — it is safe to retry. A step written as `done` means the side effect completed — it must not be re-executed. This is the execution invariant the entire system is built on. Any host, any executor, any deployment must honor it.

---

## Core Trait Boundaries

The following traits form the contract between core and host. Their exact interfaces are defined in implementation specs — this list is the architectural boundary.

| Trait | Owned by core | Implemented by host |
|---|---|---|
| `DatabaseAdapter` | step log, memory, idempotency | Postgres, SQLite, in-memory |
| `ModelProvider` | LLM call contract | OpenAI, Anthropic, Ollama, etc. |
| `EventSink` | agent event emission | SSE, WebSocket, logs, noop |
| `SchedulerHint` | wakeup / retry semantics | tokio timers, cron, external queue |

---

## Host Configurations

The same `anima-core` runs under different executors and infrastructure depending on the host. The core sees only traits — it has no knowledge of which configuration is active.

| Host | Executor | Integration | Adapters |
|---|---|---|---|
| Rust (axum) | tokio | native linking | SQLx, reqwest, SSE |
| Elixir (Phoenix) | BEAM | Rustler NIFs | Ecto, PubSub, Channels |
| TypeScript (any) | WASM event loop | wasm-bindgen | Prisma, fetch, WebSocket |
| Python | none (sync) | FFI via ctypes | SQLAlchemy, httpx |

The executor is not chosen by the core — it is brought by the host. Tokio and BEAM are peers at the same level: both are valid executors, neither is privileged by the core.

```
anima-core  (futures only, no async runtime)
      ↑
  [executor]  ←── host picks one
   /  |  \
tokio BEAM  WASM event loop  ...
```

---

## Consequences

**Positive:**
- Any language or framework can host the agent engine without forking or patching the core
- The core is independently testable with in-memory adapters — no infrastructure required
- Durable execution semantics live in the core rather than being reinvented per host, even though full restart recovery is not implemented yet
- Infrastructure choices (which DB, which HTTP framework, which async runtime) evolve independently of agent logic
- Execution semantics are defined once in the core and cannot diverge across hosts
- The embedding story is first-class: TypeScript via WASM, Python via FFI, Elixir via NIFs, Rust natively

**Negative:**
- More upfront design discipline — every infrastructure concern must be expressed as a trait before it can be used
- Host implementations must be written and maintained per target environment
- The trait boundary adds indirection; debugging across it requires understanding both sides
- WASM and FFI embeddings are architecturally supported but operationally complex — async mismatches, crash boundaries, and streaming limitations require careful handling per target
