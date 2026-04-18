# AnimaOS Kit — Product Requirements Document

**Version:** 0.2.0
**Date:** 2026-04-05
**Status:** Active

---

## 1. Product Overview

AnimaOS Kit is a Bun + TypeScript workspace built around a canonical Rust runtime for agent swarms. The Rust daemon owns execution, coordination, memory, and streaming. The TypeScript packages provide the CLI, SDK, TUI, UI, and shared contracts that developers use locally and integrate into other systems.

The primary operator surface is the terminal UI exposed through `animaos launch`. The web UI is a secondary surface and should not lead roadmap decisions until the terminal workflow is strong.

**Tagline:** Task agents that get things done.

---

## 2. Goals

- Lightweight task agents, cheap per-call, scales horizontally
- Agent-to-agent communication + dynamic spawning
- Swarm coordination (supervisor, dynamic, round-robin)
- Plugin system (Action / Provider / Evaluator)
- Model agnostic (OpenAI, Anthropic, Ollama, OpenRouter)
- BM25 search for task history + document retrieval
- Strong terminal operator experience for launch, tracing, approvals, and review
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
│   ├── @animaOS-SWARM/core      — Shared TS contracts, plugin types, and compatibility utilities
│   ├── @animaOS-SWARM/swarm     — TS swarm helpers, strategies, and shared types
│   ├── @animaOS-SWARM/tools     — TS tool registry, policies, hooks, and local executors
│   ├── @animaOS-SWARM/memory    — BM25 search, task history, and TS memory helpers
│   ├── @animaOS-SWARM/sdk       — Public TypeScript client for the Rust daemon
│   ├── @animaOS-SWARM/cli       — Local `animaos` CLI and agency scaffolding
│   └── @animaOS-SWARM/tui       — Ink-based terminal UI for launch sessions
├── hosts/rust-daemon/
│   ├── anima-core               — Canonical runtime core
│   ├── anima-swarm              — Canonical swarm coordination
│   ├── anima-memory             — Canonical memory services
│   └── anima-daemon             — Canonical HTTP/SSE execution boundary
└── apps/
    ├── @animaOS-SWARM/server    — REST API and SSE bridge for the daemon
  └── @animaOS-SWARM/ui        — Secondary web dashboard (React + Vite)
```

**Tech Stack:** Rust, Bun, TypeScript, Nx, Cargo, Vite, React, Vitest, Playwright, Oxlint

---

## 5. Current Status

### Shipping Today

| Component | Status | Details |
|---|---|---|
| Rust runtime core | ✅ Done | Reusable Rust runtime crates now live in `packages/core-rust`, while the current runnable daemon host remains under `hosts/rust-daemon/crates/anima-daemon` |
| Provider support | ✅ Done | OpenAI, Anthropic, Google/Gemini, Ollama, and several OpenAI-compatible providers run through the daemon |
| Swarm coordination | ✅ Done | Supervisor, dynamic, and round-robin strategies are implemented |
| CLI surface | ✅ Done | `create`, `run`, `chat`, `launch`, and `agents` are available through the local `animaos` binary |
| SDK | ✅ Done | The TypeScript SDK provides daemon HTTP/SSE clients plus config helper factories |
| Memory services | ✅ Done | BM25 search, task history, document storage, and recent memory retrieval are wired through the current runtime surface |
| Server API | ✅ Done | Agents, swarms, documents, search, and health endpoints are available through the server app and daemon boundary |
| TUI support | ✅ Done | Local launch sessions already render through the Ink-based terminal UI package |
| Test coverage | ✅ Done | TypeScript package tests and the Rust `cargo test` suite are part of the current development flow |

### Active Gaps

| Component | Status | Details |
|---|---|---|
| TUI operator experience | In progress | `launch` already uses the terminal UI, but it still needs stronger navigation, trace inspection, approvals, memory drill-down, and session resume |
| Web UI dashboard | Secondary | The UI app exists but remains a placeholder and is not the primary product surface |
| Auth and audit policy | Planned | API auth, managed permission tiers, and audit surfaces still need productization |
| Persistent relational storage | Planned | The current runtime leans on file/BM25 flows; a DB layer remains future work |
| Plugin packaging | Planned | Core plugin types exist, but manifests, loaders, examples, and registry conventions are not first-class yet |
| Release and install polish | Planned | CI, package docs, install paths, and changelog/security posture still need tightening |

---

## 6. Near-Term Roadmap

### Phase 1 — TUI, Observability, and Output Quality
- Make the `launch` TUI the default operator workflow worth staying inside
- Keyboard-first navigation across agents, tools, logs, and final result views
- Inline approvals, pending states, and error recovery in the terminal flow
- Session resume, recent-run browsing, and memory drill-down without leaving the terminal
- Per-agent token and cost breakdown, not just swarm totals
- Structured decision traces showing which agent ran, what tool was called, and why
- Structured JSONL output suitable for piping into external systems
- Confidence scoring contracts for agent and swarm results

### Phase 2 — Faster and Smarter Coordination
- Parallel dispatch for independent tasks
- Generate-then-validate strategies with cheap validator agents
- Smarter routing using triggers, examples, and LLM fallback
- Better shared memory and knowledge-linking between agents mid-run

### Phase 3 — Product Surface Hardening
- Harden the TUI as the primary operator surface for runs, traces, tool calls, and memory state
- Keep the web dashboard secondary until it adds clear value beyond the terminal workflow
- Auth, audit, and managed permission policy surfaces
- Storage abstractions beyond the current file-first defaults
- Better packaging, onboarding, and install flows for teams adopting the kit

### Phase 4 — Extension and Distribution Model
- First-class plugin manifests and loading conventions
- Example extensions that demonstrate actions, providers, evaluators, and policy hooks
- A clearer settings story for managed permissions and guardrails
- Bun-native release automation, changelog discipline, and distribution hygiene

---

## 7. Key Design Decisions

| Decision | Choice | Why |
|---|---|---|
| Execution runtime | Rust daemon | Keeps the canonical execution path deterministic and gives the project a clear streaming boundary |
| Primary operator surface | TUI via `animaos launch` | Fastest path to trust, iteration, and operator control without building a second product surface too early |
| Client surfaces | TypeScript packages | CLI, SDK, TUI, UI, and shared contracts stay easy to compose and integrate |
| Workspace runner | Bun | Fast installs and repo-local command execution |
| Monorepo | Nx | Project graph, affected execution, and consistent build/test orchestration |
| Plugin pattern | Action / Provider / Evaluator | Still the most coherent extension surface across the workspace |
| Swarm pattern | Supervisor / Dynamic / Round-robin | Covers the current coordination needs without overfitting to one workflow |
| Memory core | BM25 + structured persistence | Cheap search today; heavier storage remains optional |
| Tooling model | Built-in toolkit with permission controls | Keeps execution auditable while supporting local workflows and daemon execution |

---

## 8. Developer Experience Target

```ts
import { createDaemonClient, agent, swarm } from '@animaOS-SWARM/sdk';

const client = createDaemonClient();

const contentTeam = await client.swarms.create(
  swarm({
    name: 'content-team',
    strategy: 'round-robin',
    maxIterations: 6,
    agents: [
      agent({
        name: 'researcher',
        provider: 'openai',
        model: 'gpt-4o-mini',
        apiKey: process.env.OPENAI_API_KEY,
        systemPrompt: 'Gather the key facts and frame the problem clearly.',
      }),
      agent({
        name: 'writer',
        provider: 'openai',
        model: 'gpt-4o',
        apiKey: process.env.OPENAI_API_KEY,
        systemPrompt: 'Turn the research into concise, structured output.',
      }),
    ],
  })
);

const result = await client.swarms.run(contentTeam.id, {
  text: 'Write a short brief on AI agents for engineering leaders.',
});

console.log(result.result.content.text);
```

Streaming consumers can subscribe with `client.swarms.subscribe(swarmId)` and render daemon events as they arrive.

### CLI

```bash
# Scaffold a local agency without starting the daemon
animaos create content-team --provider openai --model gpt-4o-mini

# Start the daemon in another terminal, then launch work inside the agency directory
animaos launch "Write a report on AI trends"

# Run an interactive session against the daemon-backed runtime
animaos chat
```

---

## 9. Immediate Next Steps

1. Make the launch TUI the primary operator surface: navigation, trace inspection, approvals, and session resume
2. Add structured observability and confidence scoring so swarm output is auditable
3. Productize plugin, settings, and permission packaging for team use
4. Tighten release discipline with Bun-native CI, clearer install flows, and accurate docs
