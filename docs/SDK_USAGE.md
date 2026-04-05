# animaOS SDK Usage Guide

This guide covers how to interact with the animaOS SDK and daemon for building agent swarms.

## Table of Contents

- [Prerequisites](#prerequisites)
- [Architecture Overview](#architecture-overview)
- [CLI Usage](#cli-usage)
- [SDK Usage](#sdk-usage)
- [Provider Configuration](#provider-configuration)
- [Examples](#examples)

---

## Prerequisites

### 1. Build the SDK

```bash
bun run build:cli-sdk
```

### 2. Start the Daemon

The daemon must be running for SDK operations and most CLI commands:

```bash
# Terminal 1: Start the Rust daemon
bun run daemon

# Or directly with cargo
cargo run --manifest-path packages/animaos-rs/Cargo.toml -p anima-daemon
```

The daemon listens on `http://127.0.0.1:8080` by default.

### 3. Set API Keys

```bash
export OPENAI_API_KEY="sk-..."
# or other provider keys (see Provider Configuration section)
```

---

## Quick Start (CLI Only - No Code!)

You can create and use an agency directly from the CLI without writing any code:

```bash
# 1. Create an agency (no daemon required)
bun run animaos create my-agency \
  --provider openai \
  --model gpt-4o-mini \
  --api-key "$OPENAI_API_KEY"

# 2. Start the daemon (in a new terminal)
bun run daemon

# 3. Use your agency
cd my-agency
bun run animaos launch "Write a blog post about AI" --no-tui
```

That's it! The `create` command sets up everything you need.

---

## Architecture Overview

```
┌─────────────────┐     HTTP      ┌─────────────┐
│   Your Code     │ ◄───────────► │ Rust Runtime│
│   (SDK/CLI)     │   (Port 8080) │ + Daemon    │
└─────────────────┘               └──────┬──────┘
                                         │
                                    ┌────┴────┐
                                    │  LLM    │
                                    │ Providers
                                    └─────────┘
```

Runtime ownership in the current repo:

- The canonical runtime core lives in `packages/animaos-rs`.
- `@animaOS-SWARM/sdk` is a TypeScript client over the daemon's HTTP and SSE APIs.
- `@animaOS-SWARM/core` provides shared TypeScript contracts and utilities, but it is not the source of truth for runtime behavior.

---

## CLI Usage

### Commands Overview

| Command | Needs Daemon | Description |
|---------|-------------|-------------|
| `create` | ❌ | Create a new agency (local files only) |
| `launch` | ✅ | Launch a task in an agency directory |
| `run` | ✅ | Run a one-off agent task |
| `chat` | ✅ | Start interactive chat session |

### Creating an Agency

```bash
# Create a new agency (no daemon needed)
bun run animaos create my-agency \
  --provider openai \
  --model gpt-4o-mini \
  --api-key "$OPENAI_API_KEY"

# Or use environment variable (no --api-key needed)
export OPENAI_API_KEY="sk-..."
bun run animaos create my-agency --provider openai --model gpt-4o-mini
```

This creates a `my-agency/` directory with configuration files.

### Launching Tasks

```bash
cd my-agency

# Launch with TUI (interactive)
bun run animaos launch "Write a blog post about AI"

# Launch without TUI (headless)
bun run animaos launch "Write a blog post about AI" --no-tui
```

### Running One-off Tasks

```bash
cd my-agency

# Quick task without saving state
bun run animaos run "Summarize this document" --no-tui
```

### Interactive Chat

```bash
cd my-agency

# Start chat session
bun run animaos chat
```

---

## SDK Usage

### Import the SDK

```typescript
import { 
  createDaemonClient,
  agent,
  swarm,
  action,
  plugin,
  DaemonHttpError 
} from '@animaOS-SWARM/sdk';
```

### Create a Client

```typescript
// Default client (connects to localhost:8080)
const client = createDaemonClient();

// Custom configuration
const client = createDaemonClient({
  baseUrl: 'http://127.0.0.1:8080',
  // Optional: custom fetch implementation
  fetch: customFetch
});
```

### Working with Agents

#### Create an Agent

```typescript
import { agent } from '@animaOS-SWARM/sdk';

// Define agent configuration
const myAgent = agent({
  name: 'researcher',
  provider: 'openai',
  model: 'gpt-4o-mini',
  apiKey: process.env.OPENAI_API_KEY,
  systemPrompt: 'You are a research assistant specializing in technology.',
  settings: {
    temperature: 0.7,
    maxTokens: 2000
  },
  actions: []
});

// Create on daemon
const created = await client.agents.create(myAgent);
console.log('Agent ID:', created.state.id);
```

#### List All Agents

```typescript
const agents = await client.agents.list();
for (const agent of agents) {
  console.log(`${agent.state.id}: ${agent.state.name} (${agent.state.status})`);
}
```

#### Get Agent Details

```typescript
const agent = await client.agents.get('agent-uuid-here');
console.log('Status:', agent.state.status);
console.log('Messages:', agent.messageCount);
console.log('Events:', agent.eventCount);
```

#### Run an Agent

```typescript
const result = await client.agents.run('agent-uuid-here', {
  text: 'Research the latest advances in quantum computing'
});

console.log('Result:', result.result.content.text);
console.log('Usage:', result.result.usage);
```

#### Get Agent Memories

```typescript
// Get recent memories
const memories = await client.agents.recentMemories('agent-uuid-here', {
  limit: 10
});

for (const memory of memories) {
  console.log(`[${memory.importance}] ${memory.type}: ${memory.content.slice(0, 100)}...`);
}
```

### Working with Swarms

#### Create a Swarm

```typescript
import { swarm, agent } from '@animaOS-SWARM/sdk';

// Define multiple agents
const researcher = agent({
  name: 'researcher',
  provider: 'openai',
  model: 'gpt-4o-mini',
  apiKey: process.env.OPENAI_API_KEY,
  systemPrompt: 'You are a research specialist.'
});

const writer = agent({
  name: 'writer',
  provider: 'openai',
  model: 'gpt-4o',
  apiKey: process.env.OPENAI_API_KEY,
  systemPrompt: 'You are a content writer.'
});

// Create swarm
const contentTeam = swarm({
  name: 'content-team',
  agents: [researcher, writer],
  strategy: 'round-robin',
  maxIterations: 10
});

const created = await client.swarms.create(contentTeam);
console.log('Swarm ID:', created.id);
```

#### Run a Swarm

```typescript
const result = await client.swarms.run('swarm-uuid-here', {
  text: 'Write a comprehensive article about renewable energy'
});

console.log('Result:', result.result.content.text);
console.log('Iterations:', result.swarm.iteration);
```

#### Subscribe to Swarm Events (Streaming)

```typescript
// Subscribe to real-time events
const eventStream = client.swarms.subscribe('swarm-uuid-here');

for await (const event of eventStream) {
  console.log('Event:', event.event);
  console.log('Data:', event.data);
  
  // Event data includes:
  // - swarmId: string
  // - state: SwarmState
  // - result: TaskResult | null
}
```

#### Get Swarm State

```typescript
const swarmState = await client.swarms.get('swarm-uuid-here');
console.log('Active:', swarmState.active);
console.log('Iteration:', swarmState.iteration);
console.log('Agents:', swarmState.agents);
```

### Defining Actions

```typescript
import { action } from '@animaOS-SWARM/sdk';

const searchAction = action({
  name: 'web_search',
  description: 'Search the web for information',
  parameters: {
    type: 'object',
    properties: {
      query: { type: 'string', description: 'Search query' }
    },
    required: ['query']
  },
  handler: async ({ query }) => {
    // Implementation here
    return { results: [...] };
  }
});

// Add to agent
const myAgent = agent({
  name: 'researcher',
  provider: 'openai',
  model: 'gpt-4o-mini',
  apiKey: process.env.OPENAI_API_KEY,
  actions: [searchAction]
});
```

### Defining Plugins

```typescript
import { plugin } from '@animaOS-SWARM/sdk';

const memoryPlugin = plugin({
  name: 'memory',
  version: '1.0.0',
  install: (context) => {
    // Plugin setup
    return {
      onAgentStart: async (agent) => {
        console.log(`Agent ${agent.name} started`);
      },
      onAgentComplete: async (agent, result) => {
        console.log(`Agent ${agent.name} completed`);
      }
    };
  }
});
```

### Error Handling

```typescript
import { DaemonHttpError } from '@animaOS-SWARM/sdk';

try {
  const result = await client.agents.run('invalid-uuid', { text: 'Hello' });
} catch (error) {
  if (error instanceof DaemonHttpError) {
    console.error('HTTP Status:', error.status);
    console.error('Error Body:', error.body);
    console.error('Message:', error.message);
  } else {
    console.error('Unexpected error:', error);
  }
}
```

---

## Provider Configuration

### Supported Providers

| Provider | Name | Environment Variables |
|----------|------|----------------------|
| OpenAI | `openai` | `OPENAI_API_KEY`, `OPENAI_BASE_URL` |
| Anthropic | `anthropic` | `ANTHROPIC_API_KEY`, `ANTHROPIC_BASE_URL` |
| Google Gemini | `google` / `gemini` | `GOOGLE_API_KEY`, `GOOGLE_BASE_URL` |
| Ollama | `ollama` | `OLLAMA_API_KEY`, `OLLAMA_BASE_URL` |
| Groq | `groq` | `GROQ_API_KEY`, `GROQ_BASE_URL` |
| xAI | `xai` / `grok` | `XAI_API_KEY`, `XAI_BASE_URL` |
| OpenRouter | `openrouter` | `OPENROUTER_API_KEY`, `OPENROUTER_BASE_URL` |
| Mistral | `mistral` | `MISTRAL_API_KEY`, `MISTRAL_BASE_URL` |
| Together | `together` | `TOGETHER_API_KEY`, `TOGETHER_BASE_URL` |
| DeepSeek | `deepseek` | `DEEPSEEK_API_KEY`, `DEEPSEEK_BASE_URL` |
| Fireworks | `fireworks` | `FIREWORKS_API_KEY`, `FIREWORKS_BASE_URL` |
| Perplexity | `perplexity` | `PERPLEXITY_API_KEY`, `PERPLEXITY_BASE_URL` |

### Configuration Priority

1. **Inline API key** (highest priority): Passed directly in config
2. **Environment variable**: Standard env vars per provider
3. **Base URL override**: `*_BASE_URL` env vars for custom endpoints

### Example Configurations

```typescript
// OpenAI
const openaiAgent = agent({
  name: 'gpt4',
  provider: 'openai',
  model: 'gpt-4o',
  apiKey: process.env.OPENAI_API_KEY
});

// Ollama (local)
const ollamaAgent = agent({
  name: 'local',
  provider: 'ollama',
  model: 'llama3.2',
  baseUrl: 'http://localhost:11434'
});

// OpenRouter
const openrouterAgent = agent({
  name: 'multi',
  provider: 'openrouter',
  model: 'anthropic/claude-3-opus',
  apiKey: process.env.OPENROUTER_API_KEY
});
```

---

## Examples

### Example 1: Simple Research Agent

```typescript
import { createDaemonClient, agent } from '@animaOS-SWARM/sdk';

const client = createDaemonClient();

async function main() {
  // Create agent
  const researcher = await client.agents.create(agent({
    name: 'researcher',
    provider: 'openai',
    model: 'gpt-4o-mini',
    apiKey: process.env.OPENAI_API_KEY,
    systemPrompt: 'You are a research assistant. Provide concise, factual answers.'
  }));

  // Run task
  const result = await client.agents.run(researcher.state.id, {
    text: 'What are the main types of neural networks?'
  });

  console.log('Answer:', result.result.content.text);
  console.log('Tokens used:', result.result.usage?.total);
}

main().catch(console.error);
```

### Example 2: Multi-Agent Content Team

```typescript
import { createDaemonClient, agent, swarm } from '@animaOS-SWARM/sdk';

const client = createDaemonClient();

async function createContentTeam() {
  // Create swarm with multiple agents
  const contentSwarm = await client.swarms.create(swarm({
    name: 'blog-team',
    strategy: 'round-robin',
    agents: [
      agent({
        name: 'researcher',
        provider: 'openai',
        model: 'gpt-4o-mini',
        apiKey: process.env.OPENAI_API_KEY,
        systemPrompt: 'Research topics and provide key points. Be thorough.'
      }),
      agent({
        name: 'writer',
        provider: 'openai',
        model: 'gpt-4o',
        apiKey: process.env.OPENAI_API_KEY,
        systemPrompt: 'Write engaging content based on research. Use markdown.'
      }),
      agent({
        name: 'editor',
        provider: 'openai',
        model: 'gpt-4o-mini',
        apiKey: process.env.OPENAI_API_KEY,
        systemPrompt: 'Review and polish content. Fix grammar and improve flow.'
      })
    ]
  }));

  // Run with streaming
  const stream = client.swarms.subscribe(contentSwarm.id);
  
  // Run task
  const runPromise = client.swarms.run(contentSwarm.id, {
    text: 'Create a blog post about the future of AI agents'
  });

  // Process events
  for await (const event of stream) {
    if (event.data.result) {
      console.log(`[${event.data.swarmId}] Progress:`, event.data.state.iteration);
    }
  }

  const result = await runPromise;
  console.log('Final content:', result.result.content.text);
}

createContentTeam().catch(console.error);
```

### Example 3: Agent with Memory

```typescript
import { createDaemonClient, agent } from '@animaOS-SWARM/sdk';

const client = createDaemonClient();

async function conversationalAgent() {
  const assistant = await client.agents.create(agent({
    name: 'assistant',
    provider: 'openai',
    model: 'gpt-4o-mini',
    apiKey: process.env.OPENAI_API_KEY,
    systemPrompt: 'You are a helpful assistant with memory of past conversations.'
  }));

  // First interaction
  await client.agents.run(assistant.state.id, {
    text: 'My name is Alice and I love Python programming.'
  });

  // Second interaction - should remember
  const result = await client.agents.run(assistant.state.id, {
    text: 'What programming language do I like?'
  });

  console.log('Response:', result.result.content.text);
  // Should mention Python based on memory

  // Check memories
  const memories = await client.agents.recentMemories(assistant.state.id, { limit: 5 });
  console.log('Stored memories:', memories.length);
}

conversationalAgent().catch(console.error);
```

---

## Troubleshooting

### "Connection refused" Error

The daemon is not running. Start it with:
```bash
bun run daemon
```

### "Invalid API key" Error

Check that your API key is set correctly:
```bash
echo $OPENAI_API_KEY  # Should show your key
```

### "Agent not found" Error

Verify the agent ID exists:
```typescript
const agents = await client.agents.list();
console.log(agents.map(a => a.state.id));
```

---

## Additional Resources

- [Project README](../README.md) - Overview and quick start
- [Design Docs](./design/) - Architecture and design decisions
- [Rust Workspace](../packages/animaos-rs/) - Canonical runtime core and daemon crates
- [Core Package](../packages/core/) - Shared TypeScript contracts and utilities
- [SDK Source](../packages/sdk/src/) - TypeScript daemon client implementation
