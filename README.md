# animaOS-SWARM

Agent swarm framework. Command and control your AI agents -- spawn, coordinate, and manage swarms that get things done.

## Features

- **Agent Runtime** -- LLM loop with tool calling, token tracking, and plugin registration
- **Swarm Coordination** -- Supervisor, dynamic, and round-robin strategies
- **Agent-to-Agent Communication** -- Direct messaging, broadcast, dynamic spawning
- **Plugin System** -- Action / Provider / Evaluator pattern
- **Model Agnostic** -- OpenAI, Anthropic, Ollama adapters included
- **BM25 Search** -- Task history and document retrieval with zero token cost
- **Built-in Tools** -- bash, read, write, edit, grep, glob, process manager

## Architecture

```
animaos-swarm/
├── packages/
│   ├── core       — Agent runtime, types, model adapters, plugin system
│   ├── swarm      — Swarm coordinator, strategies, agent-to-agent messaging
│   ├── tools      — Tool registry + executor with permission checks
│   ├── memory     — BM25 search, task history, document ingestion
│   ├── sdk        — Public SDK that re-exports clean developer API
│   └── cli        — CLI commands (run, chat)
└── apps/
    ├── server     — REST API server
    └── ui         — Web dashboard (React + Vite)
```

## Quick Start

```bash
bun install
```

### Run a single task

```bash
animaos-swarm run "What is 42 * 17?" --model gpt-4o-mini
```

### Interactive chat

```bash
animaos-swarm chat --model gpt-4o
```

### Run a swarm

```bash
animaos-swarm swarm run --strategy supervisor "Write a report on AI trends"
```

## Usage

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
  tools: [tools.draft],
})

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

// Spawn child agent
const analyst = await agent.spawn({
  role: "analyst",
  tools: [tools.queryDB],
  task: "Analyze this dataset",
})
```

## Development

```bash
# Build all packages
bun nx run-many -t build

# Run tests
bun nx run-many -t test

# Start the server
bun nx serve server

# Start the UI
bun nx dev ui
```

## Tech Stack

Bun, TypeScript, Nx, Vitest, Vite, React

## License

MIT
