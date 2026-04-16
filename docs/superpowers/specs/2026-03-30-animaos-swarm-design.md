# animaOS-SWARM Design Spec

**Date:** 2026-03-30
**Status:** Approved

---

## Vision

Command and control for AI agent swarms. One command to launch a swarm, watch agents coordinate in real-time through a TUI or web dashboard. Zero config to start, full config when you need control.

---

## Architecture

Three layers: Runtime, Server, Interfaces.

### Layer 1: Runtime (exists -- fix and strengthen)

The agent runtime, swarm coordinator, tools, memory, and model adapters are already implemented. Before building on top:

- Fix broken SDK import (`allToolActions` should be `ALL_TOOL_ACTIONS` in `packages/sdk/src/index.ts`)
- Add OpenAI adapter `generateStream` (Anthropic and Ollama already have it)
- Add tests for swarm and tools packages (currently zero test coverage)
- Update PRD roadmap checkboxes to match actual status

### Layer 2: Server (the brain)

REST API + WebSocket server. Both TUI and web dashboard connect here.

**WebSocket events** -- every agent lifecycle event streams out in real-time:

- `agent:spawned`, `agent:terminated`
- `agent:thinking`, `agent:tool_call`, `agent:tool_result`
- `agent:message_sent`, `agent:message_received`
- `agent:status_changed` (idle / thinking / running_tool / done / error)
- `swarm:started`, `swarm:completed`, `swarm:error`
- `token:usage` (per agent, running totals)

**Persistence** -- Drizzle ORM + pglite (default), Postgres for production:

- Swarm runs (config, status, result)
- Agent instances (config, status, token usage)
- Message history (sender, receiver, content, timestamp)
- Tool call log (agent, tool, args, result, duration)
- Task history (searchable via existing BM25)

### Layer 3: Interfaces

Three ways to control the swarm, all talking to the same runtime/server:

#### CLI Commands

```bash
# Run a swarm (launches TUI)
animaos-swarm run "Write a blog post about AI agents"
animaos-swarm run "do the thing" --strategy supervisor --model gpt-4o
animaos-swarm run "do the thing" --agents ./my-swarm.yaml

# Management
animaos-swarm status                    # show running swarms
animaos-swarm agents list               # list active agents
animaos-swarm stop <swarm-id>           # stop a swarm
animaos-swarm logs <agent-id>           # view agent logs

# Interactive
animaos-swarm chat --model gpt-4o       # single agent chat (exists)
```

#### TUI (Terminal Mission Control)

Launches automatically with `animaos-swarm run`. Shows:

```
 SWARM -- supervisor -- 3 agents

 [supervisor]  thinking...     tokens: 340
 [researcher]  running tool    tokens: 1,204
 [writer]      idle            tokens: 0

 -- messages -----------------------------------------------
 supervisor -> researcher: "Research current AI agent frameworks"
 researcher -> supervisor: "Found 5 key trends..."
 supervisor -> writer: "Draft a post covering these trends..."
 writer -> supervisor: "Here's the draft..."

 -- tools --------------------------------------------------
 researcher: web_search("AI agent frameworks 2026")
 researcher: scrape(url: "...")

 tokens: 3,847 total | cost: $0.04 | elapsed: 34s
```

Components:

- **Agent panel** -- list of agents with live status (idle / thinking / running tool / done), token count per agent
- **Message stream** -- real-time feed of agent-to-agent messages (who -> who, content)
- **Tool panel** -- tool calls in progress and completed (agent, tool name, args, result)
- **Status bar** -- total tokens, estimated cost, elapsed time
- **Timeline/log** -- scrollable log of all events

#### Web Dashboard

Browser-based version of the TUI with richer visuals:

- Agent graph visualization (nodes = agents, edges = message flow)
- Message flow diagram
- Searchable task history
- Settings (API keys, model config)

---

## Zero Config -> Full Config

### Zero config

```bash
animaos-swarm run "Write a blog post"
```

- Uses a default supervisor strategy
- Spawns agents dynamically based on the task
- Requires only an API key (env var or `~/.animaos-swarm/config.yaml`)
- Default model: whatever is configured

### Config file

```yaml
# swarm.yaml
strategy: supervisor
model: gpt-4o

agents:
  researcher:
    system: "You research topics thoroughly."
    tools: [web_search, scrape]

  writer:
    system: "You write clear, concise content."
    tools: [draft]
```

```bash
animaos-swarm run "Write a blog post" --agents ./swarm.yaml
```

### Programmatic (SDK)

```ts
import { agent, swarm, tools } from "@animaOS-SWARM/sdk"

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
})

const mySwarm = swarm({
  strategy: "supervisor",
  manager: researcher,
  workers: [writer],
})

await mySwarm.run("Write a blog post about AI agents")
```

---

## Build Order

### Phase 1: Fix Foundations

- Fix SDK broken import
- Add `generateStream` to OpenAI adapter
- Add tests for swarm package
- Add tests for tools package
- Clean up PRD roadmap

### Phase 2: WebSocket Events

- Wire EventBus to emit all lifecycle events
- Add WebSocket server alongside REST API
- Define event protocol (JSON messages with type, agentId, timestamp, payload)
- Client-side event consumer (shared between TUI and dashboard)

### Phase 3: TUI

- Agent panel component (status, tokens)
- Message stream component (real-time feed)
- Tool panel component (tool calls)
- Status bar component (totals, cost, elapsed)
- Timeline/log component (scrollable)
- Keyboard navigation (switch panels, scroll, quit)

### Phase 4: CLI Commands

- `run` command with TUI integration (replace current basic output)
- `status` command
- `agents list` command
- `stop` command
- `logs` command
- Zero config mode (default strategy, dynamic agent spawning)
- Config file loading (`--agents ./swarm.yaml`)

### Phase 5: Persistence

- Drizzle ORM setup with pglite
- Schema: swarm_runs, agents, messages, tool_calls
- Save all events to DB during runs
- Query API for history

### Phase 6: Web Dashboard

- React + Vite (app already scaffolded)
- Connect to WebSocket for live data
- Agent list view
- Message flow view
- Tool call log
- Task history (searchable)

### Phase 7: Auth + Polish

- API key auth
- Config file (`~/.animaos-swarm/config.yaml`) for API keys, default model
- Error handling and edge cases
- Documentation

---

## Key Decisions

| Decision | Choice | Why |
|---|---|---|
| TUI framework | Custom (like elizaOS) | Full control, no heavy dependency |
| Real-time protocol | WebSocket | Bidirectional, low latency, standard |
| Default DB | pglite | Zero setup, embedded, swappable |
| Config format | YAML | Human-readable, industry standard for config |
| Event model | EventBus -> WebSocket broadcast | Already have EventBus, just pipe it out |
