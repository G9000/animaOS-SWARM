# animaOS Rust Workspace

The runtime core for AnimaOS. This workspace owns agent execution, swarm coordination, memory services, and the HTTP daemon that bridges everything to TypeScript clients.

```mermaid
graph BT
    anima-core["anima-core\n(execution engine, traits)"]
    anima-memory["anima-memory\n(BM25 search, storage)"]
    anima-swarm["anima-swarm\n(multi-agent coordination)"]
    anima-daemon["anima-daemon\n(HTTP API, Postgres, SSE)"]

    anima-memory --> anima-core
    anima-swarm --> anima-core
    anima-daemon --> anima-core
    anima-daemon --> anima-memory
    anima-daemon --> anima-swarm
```

## Crates

| Crate | What it does |
|---|---|
| [`anima-core`](crates/anima-core) | Agent execution loop, trait interfaces (ModelAdapter, DatabaseAdapter, Provider, Evaluator). No HTTP, no DB, no runtime dep. |
| [`anima-memory`](crates/anima-memory) | BM25 full-text memory store with optional JSON file persistence. Synchronous, no external dependencies. |
| [`anima-swarm`](crates/anima-swarm) | Multi-agent coordinator with Supervisor, RoundRobin, and Dynamic strategies. Built on top of anima-core agents. |
| [`anima-daemon`](crates/anima-daemon) | Axum HTTP server. Implements ModelAdapter + DatabaseAdapter from anima-core, exposes the REST API, emits SSE events. |

## Quick start

```bash
# from the repo root
cd packages/animaos-rs

ANTHROPIC_API_KEY=sk-... cargo run -p anima-daemon
curl http://127.0.0.1:8080/health
```

With Postgres step persistence:

```bash
DATABASE_URL=postgres://user:pass@localhost/animaos \
ANTHROPIC_API_KEY=sk-... \
cargo run -p anima-daemon
```

## Build

```bash
cargo build --workspace
```

## Test

```bash
# unit + integration (requires no external services)
cargo test --workspace

# integration tests that need Postgres
DATABASE_URL=postgres://user:pass@localhost/animaos cargo test --workspace
```

## How it fits together

TypeScript clients (SDK, CLI, UI) talk exclusively to `anima-daemon` over HTTP. The daemon wires the pure `anima-core` runtime to real infrastructure — Anthropic's API for the model and Postgres for durable step logging. `anima-memory` and `anima-swarm` are used by the daemon but are independently testable without it.
