# AnimaOS Kit — Product Requirements Document

**Version:** 0.1.0
**Date:** 2026-03-30
**Status:** In Progress

---

## 1. Product Overview

AnimaOS Kit is a lightweight TypeScript agent swarm framework for enterprise use. It is the enterprise arm of AnimaOS — task-focused, token-efficient, and horizontally scalable. Agents can talk to each other, delegate tasks, spawn new agents dynamically, and coordinate via swarm strategies.

**Tagline:** Task agents that get things done.

---

## 2. Goals

- Lightweight task agents, cheap per-call, scales horizontally
- Agent-to-agent communication + dynamic spawning
- Swarm coordination (supervisor, dynamic, round-robin)
- Plugin system (Action / Provider / Evaluator)
- Model agnostic (OpenAI, Anthropic, Ollama, OpenRouter)
- BM25 search for task history + document retrieval
- Web dashboard for agent management
- Enterprise-ready (auth, audit, permissions)

## 3. Non-Goals

- No deep memory (no consolidation, compaction, forgetting, heat scoring)
- No cognitive loop / inner monologue
- No persona / soul / emotional state
- No companion features

---

## 4. Architecture

```
animaos-swarm/
├── packages/
│   ├── @animaOS-SWARM/core      — Shared TS contracts, adapters, and compatibility utilities
│   ├── @animaOS-SWARM/swarm     — Swarm coordinator, strategies, agent-to-agent messaging
│   ├── @animaOS-SWARM/tools     — Tool registry (bash, read, write, grep, glob, etc.)
│   ├── @animaOS-SWARM/memory    — BM25 search, task history, document ingestion
│   ├── @animaOS-SWARM/sdk       — Public TypeScript SDK for the Rust daemon
│   └── @animaOS-SWARM/cli       — CLI commands (run, chat, create, agents, swarm)
├── packages/animaos-rs/
│   ├── anima-core               — Canonical runtime core
│   ├── anima-swarm              — Canonical swarm coordination
│   ├── anima-memory             — Canonical memory services
│   └── anima-daemon             — Canonical HTTP/SSE execution boundary
└── apps/
    ├── @animaOS-SWARM/server    — REST API + WebSocket server
    └── @animaOS-SWARM/ui        — Web dashboard (React + Vite)
```

**Tech Stack:** Rust, Bun, TypeScript, NX (monorepo), Cargo, Vitest, Biome, Drizzle ORM, pglite (default DB)

---

## 5. Current Status

### Done

| Component | Status | Details |
|---|---|---|
| Monorepo scaffold | ✅ Done | NX + Bun, all packages and apps created |
| Rust runtime core | ✅ Done | `anima-core`, `anima-swarm`, `anima-memory`, and `anima-daemon` are the canonical execution path |
| TypeScript shared core | ✅ Done | `@animaOS-SWARM/core` still provides shared TS contracts and utilities for client-side packages |
| Runtime events | ✅ Done | Canonical lifecycle and SSE event flow now comes from the Rust daemon |
| OpenAI adapter | ✅ Done | Full tool-call support |
| Anthropic adapter | ✅ Done | Claude tool_use support + streaming |
| Ollama adapter | ✅ Done | OpenAI-compatible API, no key required |
| Swarm coordinator | ✅ Done | Registry, lifecycle, message routing, dynamic spawning |
| Swarm strategies | ✅ Done | Supervisor, dynamic, round-robin |
| Message bus | ✅ Done | Direct send + broadcast, inbox per agent |
| Tools package | ✅ Done | bash, read, write, edit, grep, glob, multi-edit, todo, process manager |
| Tool executor | ✅ Done | Permission checks, hooks, secrets, validation, truncation |
| BM25 search | ✅ Done | Custom BM25 with stemming, 12 tests passing |
| Task history | ✅ Done | Record, search, getRecent, getByAgent |
| Document store | ✅ Done | Ingest, chunk, search via BM25 |
| CLI (run + chat) | ✅ Done | Single task execution + interactive chat |
| SDK | ✅ Done | TypeScript daemon client and shared type surface |
| Server (REST API) | ✅ Done | Agents, swarms, search, documents, health endpoints |
| Helper factories | ✅ Done | `agent()`, `plugin()`, `action()`, `swarm()` |
| Tests | ✅ Done | 21 tests passing (core + memory) |

### Not Started

| Component | Status |
|---|---|
| Web UI dashboard | ❌ |
| WebSocket real-time events | ❌ |
| CLI: create, agents, swarm commands | ❌ |
| Auth (API key + JWT) | ❌ |
| Database layer (Drizzle + pglite) | ❌ |
| Streaming responses end-to-end | ❌ |

---

## 6. Roadmap

### Phase 1 — Core Agent Loop (DONE)
- [x] Types and interfaces
- [x] Agent runtime with tool loop
- [x] Event bus
- [x] OpenAI adapter
- [x] CLI (run + chat)
- [x] Unit tests

### Phase 2 — Swarm Coordination
- [ ] Swarm coordinator (registry, lifecycle, message routing)
- [ ] Message bus (direct messaging + broadcast)
- [ ] Supervisor strategy (manager delegates to workers)
- [ ] Dynamic strategy (LLM decides who speaks next)
- [ ] Round-robin strategy (agents take turns)
- [ ] Dynamic agent spawning (spawn → task → terminate)
- [ ] Token budget + timeout per agent
- [ ] Swarm-level tests

### Phase 3 — Tools & Memory
- [ ] Tools (bash, read, write, edit, grep, glob, process manager)
- [ ] Tool executor with permission checks + hooks
- [ ] BM25 search engine
- [ ] Task history storage
- [ ] Document ingestion + chunking

### Phase 4 — Model Adapters
- [ ] Anthropic adapter (Claude)
- [ ] Ollama adapter (local models)
- [ ] OpenRouter adapter
- [ ] Streaming support for all adapters

### Phase 5 — Server & API
- [ ] REST API (agents, swarms, tasks, search)
- [ ] WebSocket for real-time events
- [ ] Database layer (Drizzle ORM + pglite default, Postgres adapter)
- [ ] Auth (API key + JWT)
- [ ] Audit logging

### Phase 6 — Web Dashboard
- [ ] Agent list view (status, token usage)
- [ ] Swarm visualization (agent graph, message flow)
- [ ] Real-time logs (streaming output, tool calls)
- [ ] Task history (searchable past runs)
- [ ] Settings (API keys, model config)

### Phase 7 — SDK & Developer Experience
- [ ] `@animaOS-SWARM/sdk` — clean public API surface
- [ ] CLI: `animaos-swarm create` (project scaffolding)
- [ ] CLI: `animaos-swarm agents list/spawn/terminate`
- [ ] CLI: `animaos-swarm swarm run --strategy supervisor`
- [ ] Plugin marketplace / registry

### Phase 8 — Enterprise
- [ ] Organization management
- [ ] Per-agent permission scopes
- [ ] Rate limiting + token budgets
- [ ] Embeddings + vector search (optional plugin)
- [ ] Postgres adapter for production

---

## 7. Key Design Decisions

| Decision | Choice | Why |
|---|---|---|
| Language | TypeScript | Larger ecosystem, enterprise-friendly |
| Runtime | Bun | Fast, built-in TypeScript, good DX |
| Monorepo | NX | Better than Turbo for large workspaces, dependency graph |
| Database | pglite (default), Postgres (production) | Zero setup for dev, swappable adapter |
| Plugin pattern | Action / Provider / Evaluator | Proven, consistent, easy to extend |
| Swarm pattern | Supervisor / Dynamic / Round-robin | Covers all coordination needs |
| Coordinator model | Coordinator owns all agents | Full observability, no runaway agents |
| Memory | BM25 only (core), embeddings optional | Cheap, zero token cost for search |
| Tools | Built-in toolkit | Stable, tested TypeScript |

---

## 8. Developer Experience Target

```ts
import { agent, swarm, tools } from "@animaOS-SWARM/sdk"

// Define agents
const researcher = agent({
  name: "researcher",
  model: "gpt-4o",
  system: "You research topics thoroughly.",
  tools: [tools.webSearch, tools.scrape],
})

const writer = agent({
  name: "writer",
  model: "gpt-4o",
  system: "You write clear, concise content.",
  tools: [tools.draft],
})

// Wire into swarm
const mySwarm = swarm({
  strategy: "supervisor",
  manager: researcher,
  workers: [writer],
})

await mySwarm.run("Write a blog post about AI agents")
```

### Agent-to-agent communication

```ts
// Direct message
await agent.send("writer", { text: "draft the intro", metadata: { context: data } })

// Broadcast
await agent.broadcast({ text: "research complete" })

// Spawn child
const analyst = await agent.spawn({
  role: "analyst",
  tools: [tools.queryDB],
  task: "Analyze this dataset",
})
```

### CLI

```bash
# Single task
animaos-swarm run "What is 42 * 17?" --model gpt-4o-mini

# Interactive chat
animaos-swarm chat --model gpt-4o

# Swarm task
animaos-swarm swarm run --strategy supervisor "Write a report on AI trends"
```

---

## 9. Immediate Next Steps

1. **Phase 2: Swarm coordinator** — this is the key differentiator
2. **Phase 3: Port tools from AnimaOS** — free reuse, stable code
3. **Phase 4: Anthropic adapter** — model diversity
4. **Test end-to-end** — run a real swarm with multiple agents coordinating
