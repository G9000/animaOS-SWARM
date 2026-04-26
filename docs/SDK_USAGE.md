# animaOS SDK Usage Guide

This guide covers the current public TypeScript SDK surface for talking to the Rust daemon host and building agent or swarm workflows against it.

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
cargo run --manifest-path Cargo.toml -p anima-daemon
```

The daemon listens on `http://127.0.0.1:8080` by default.

### 3. Set Provider Credentials

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

That's it. The `create` command sets up the local files, and the daemon-backed commands talk to the runnable host in `hosts/rust-daemon`.

---

## Architecture Overview

```text
┌─────────────────┐     HTTP/SSE     ┌────────────────────┐
│    Your Code    │ ───────────────► │   rust-daemon host │
│   (SDK / CLI)   │                  │    on port 8080    │
└─────────────────┘                  └─────────┬──────────┘
                                               │
                                      ┌────────┴─────────┐
                                      │ packages/core-rust│
                                      │ reusable runtime  │
                                      └────────┬─────────┘
                                               │
                                      ┌────────┴─────────┐
                                      │  LLM providers   │
                                      └──────────────────┘
```

Runtime ownership in the current repo:

- The reusable Rust runtime core lives in `packages/core-rust`.
- The runnable Rust daemon host lives in `hosts/rust-daemon`.
- `@animaOS-SWARM/sdk` is a TypeScript client over the daemon's HTTP and SSE APIs.
- `@animaOS-SWARM/core` is the TypeScript core port in `packages/core-ts`; it provides shared contracts and typed helpers such as `agent()`, `action()`, and `plugin()`.

---

## CLI Usage

### Commands Overview

| Command | Needs Daemon | Description |
|---------|-------------|-------------|
| `create` | No | Create a new agency (local files only) |
| `launch` | Yes | Launch a task in an agency directory |
| `run` | Yes | Run a one-off agent or swarm task |
| `chat` | Yes | Start an interactive chat session |

### Creating an Agency

```bash
# Create a new agency (no daemon needed)
bun run animaos create my-agency \
  --provider openai \
  --model gpt-4o-mini \
  --api-key "$OPENAI_API_KEY"
```

### Launching Tasks

```bash
cd my-agency

# Launch with TUI
bun run animaos launch "Write a blog post about AI"

# Launch without TUI
bun run animaos launch "Write a blog post about AI" --no-tui
```

### Running One-off Tasks

```bash
cd my-agency
bun run animaos run "Summarize this document" --no-tui
```

### Interactive Chat

```bash
cd my-agency
bun run animaos chat
```

---

## SDK Usage

### Import the SDK

```typescript
import {
  action,
  agent,
  createDaemonClient,
  DaemonHttpError,
  plugin,
  swarm,
} from '@animaOS-SWARM/sdk';
```

### Create a Client

```typescript
const client = createDaemonClient();

const customClient = createDaemonClient({
  baseUrl: 'http://127.0.0.1:8080',
  fetch: customFetch,
});
```

### Working with Agents

`agent()` is a typed helper over the live `AgentConfig` surface. The current public agent shape uses `system`, `tools`, and `settings`.

#### Create an Agent

```typescript
import { action, agent } from '@animaOS-SWARM/sdk';

const memorySearch = action({
  name: 'memory_search',
  description: 'Search stored memories by keyword',
  parameters: {
    query: { type: 'string', description: 'Search query' },
    limit: { type: 'number', description: 'Maximum matches to return' },
  },
  handler: async (_runtime, _message, args) => {
    const query = typeof args.query === 'string' ? args.query : '';
    return {
      status: 'success',
      data: { text: `Search requested for: ${query}` },
      durationMs: 0,
    };
  },
});

const myAgent = agent({
  name: 'researcher',
  provider: 'openai',
  model: 'gpt-4o-mini',
  system: 'You are a research assistant specializing in technology.',
  tools: [memorySearch],
  settings: {
    temperature: 0.7,
    maxTokens: 2000,
    apiKey: process.env.OPENAI_API_KEY,
  },
});

const created = await client.agents.create(myAgent);
console.log('Agent ID:', created.state.id);
console.log('Status:', created.state.status);
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
const snapshot = await client.agents.get('agent-uuid-here');
console.log('Status:', snapshot.state.status);
console.log('Messages:', snapshot.messageCount);
console.log('Events:', snapshot.eventCount);
console.log('Last task:', snapshot.lastTask?.status);
```

#### Run an Agent

```typescript
const run = await client.agents.run('agent-uuid-here', {
  text: 'Research the latest advances in quantum computing',
});

if (
  run.result.status === 'success' &&
  typeof run.result.data === 'object' &&
  run.result.data !== null &&
  'text' in run.result.data
) {
  console.log('Result:', run.result.data.text);
} else {
  console.error('Run failed:', run.result.error);
}

console.log('Duration:', run.result.durationMs);
console.log('Tokens:', run.agent.state.tokenUsage.totalTokens);
```

#### Get Agent Memories

```typescript
const memories = await client.agents.recentMemories('agent-uuid-here', {
  limit: 10,
});

for (const memory of memories) {
  console.log(`[${memory.importance}] ${memory.type}: ${memory.content}`);
}
```

### Working with Swarms

`swarm()` is a typed helper over the live `SwarmConfig` surface. The current public swarm shape uses `manager`, `workers`, and optional limits such as `maxTurns`.

#### Create a Swarm

```typescript
import { agent, swarm } from '@animaOS-SWARM/sdk';

const modelSettings = {
  apiKey: process.env.OPENAI_API_KEY,
};

const contentTeam = swarm({
  strategy: 'round-robin',
  maxTurns: 10,
  manager: agent({
    name: 'manager',
    provider: 'openai',
    model: 'gpt-4o',
    system: 'Break complex tasks into subtasks and synthesize the final answer.',
    settings: modelSettings,
  }),
  workers: [
    agent({
      name: 'researcher',
      provider: 'openai',
      model: 'gpt-4o-mini',
      system: 'Research the assigned topic and return concise findings.',
      settings: modelSettings,
    }),
    agent({
      name: 'writer',
      provider: 'openai',
      model: 'gpt-4o',
      system: 'Turn approved findings into polished written output.',
      settings: modelSettings,
    }),
  ],
});

const created = await client.swarms.create(contentTeam);
console.log('Swarm ID:', created.id);
console.log('Status:', created.status);
```

#### Run a Swarm

```typescript
const run = await client.swarms.run('swarm-uuid-here', {
  text: 'Write a comprehensive article about renewable energy',
});

if (
  run.result.status === 'success' &&
  typeof run.result.data === 'object' &&
  run.result.data !== null &&
  'text' in run.result.data
) {
  console.log('Result:', run.result.data.text);
} else {
  console.error('Run failed:', run.result.error);
}

console.log('Status:', run.swarm.status);
console.log('Agents involved:', run.swarm.agentIds);
console.log('Total tokens:', run.swarm.tokenUsage.totalTokens);
```

#### Subscribe to Swarm Events (Streaming)

```typescript
const stream = client.swarms.subscribe('swarm-uuid-here');

for await (const event of stream) {
  console.log('Event:', event.event);
  console.log('Data:', event.data);
}
```

#### Get Swarm State

```typescript
const swarmState = await client.swarms.get('swarm-uuid-here');
console.log('Status:', swarmState.status);
console.log('Agents:', swarmState.agentIds);
console.log('Total tokens:', swarmState.tokenUsage.totalTokens);
console.log('Started at:', swarmState.startedAt);
console.log('Completed at:', swarmState.completedAt);
```

### Working with Memories

```typescript
await client.memories.create({
  agentId: 'agent-uuid-here',
  agentName: 'researcher',
  type: 'fact',
  content: 'Rust daemon memory endpoint created',
  importance: 0.8,
  tags: ['sdk', 'memory'],
});

const searchResults = await client.memories.search('daemon memory', {
  agentName: 'researcher',
  type: 'fact',
  limit: 5,
  minImportance: 0.5,
});

const recent = await client.memories.recent({
  agentId: 'agent-uuid-here',
  limit: 3,
});
```

### Defining Actions

```typescript
import { action } from '@animaOS-SWARM/sdk';

const searchAction = action({
  name: 'web_search',
  description: 'Search the web for information',
  parameters: {
    query: { type: 'string', description: 'Search query' },
  },
  handler: async (_runtime, _message, args) => {
    const query = typeof args.query === 'string' ? args.query : '';
    return {
      status: 'success',
      data: { text: `Results for ${query}` },
      durationMs: 0,
    };
  },
});
```

### Defining Plugins

```typescript
import { plugin } from '@animaOS-SWARM/sdk';

const notesPlugin = plugin({
  name: 'notes',
  description: 'Registers startup and cleanup hooks for note-oriented agents',
  init: async (runtime) => {
    console.log(`Plugin initialized for ${runtime.agentId}`);
  },
  cleanup: async (runtime) => {
    console.log(`Plugin cleaned up for ${runtime.agentId}`);
  },
});
```

### Error Handling

```typescript
import { DaemonHttpError } from '@animaOS-SWARM/sdk';

try {
  await client.agents.run('invalid-uuid', { text: 'Hello' });
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
| Moonshot Kimi | `moonshot` / `kimi` | `MOONSHOT_API_KEY`, `MOONSHOT_BASE_URL` |

### Configuration Priority

1. Per-agent or per-swarm `settings` for model-specific overrides such as `apiKey`, `baseUrl`, `temperature`, or `maxTokens`
2. Daemon environment variables such as `OPENAI_API_KEY` or `OPENAI_BASE_URL`
3. SDK transport settings such as `createDaemonClient({ baseUrl })`, which control where HTTP and SSE requests are sent

### Example Configurations

```typescript
const openaiAgent = agent({
  name: 'gpt4',
  provider: 'openai',
  model: 'gpt-4o',
  settings: {
    apiKey: process.env.OPENAI_API_KEY,
  },
});

const ollamaAgent = agent({
  name: 'local',
  provider: 'ollama',
  model: 'llama3.2',
  settings: {
    baseUrl: 'http://localhost:11434',
  },
});

const openrouterAgent = agent({
  name: 'multi',
  provider: 'openrouter',
  model: 'anthropic/claude-3-opus',
  settings: {
    apiKey: process.env.OPENROUTER_API_KEY,
  },
});
```

---

## Examples

### Example 1: Simple Research Agent

```typescript
import { agent, createDaemonClient } from '@animaOS-SWARM/sdk';

const client = createDaemonClient();

async function main() {
  const researcher = await client.agents.create(
    agent({
      name: 'researcher',
      provider: 'openai',
      model: 'gpt-4o-mini',
      system: 'You are a research assistant. Provide concise, factual answers.',
      settings: {
        apiKey: process.env.OPENAI_API_KEY,
      },
    })
  );

  const run = await client.agents.run(researcher.state.id, {
    text: 'What are the main types of neural networks?',
  });

  if (
    run.result.status === 'success' &&
    typeof run.result.data === 'object' &&
    run.result.data !== null &&
    'text' in run.result.data
  ) {
    console.log('Answer:', run.result.data.text);
  } else {
    console.error('Run failed:', run.result.error);
  }

  console.log('Tokens used:', run.agent.state.tokenUsage.totalTokens);
}

main().catch(console.error);
```

### Example 2: Multi-Agent Content Team

```typescript
import { agent, createDaemonClient, swarm } from '@animaOS-SWARM/sdk';

const client = createDaemonClient();

async function createContentTeam() {
  const modelSettings = {
    apiKey: process.env.OPENAI_API_KEY,
  };

  const contentSwarm = await client.swarms.create(
    swarm({
      strategy: 'round-robin',
      maxTurns: 6,
      manager: agent({
        name: 'manager',
        provider: 'openai',
        model: 'gpt-4o',
        system: 'Delegate work to specialists and synthesize the final answer.',
        settings: modelSettings,
      }),
      workers: [
        agent({
          name: 'researcher',
          provider: 'openai',
          model: 'gpt-4o-mini',
          system: 'Research topics and provide key points. Be thorough.',
          settings: modelSettings,
        }),
        agent({
          name: 'writer',
          provider: 'openai',
          model: 'gpt-4o',
          system: 'Write engaging content based on research. Use markdown.',
          settings: modelSettings,
        }),
      ],
    })
  );

  const stream = client.swarms.subscribe(contentSwarm.id);
  const runPromise = client.swarms.run(contentSwarm.id, {
    text: 'Create a blog post about the future of AI agents',
  });

  for await (const event of stream) {
    if (event.event === 'swarm:completed') {
      break;
    }

    if (
      typeof event.data === 'object' &&
      event.data !== null &&
      'state' in event.data &&
      typeof event.data.state === 'object' &&
      event.data.state !== null &&
      'status' in event.data.state
    ) {
      console.log('Swarm status:', event.data.state.status);
    }
  }

  const run = await runPromise;
  if (
    run.result.status === 'success' &&
    typeof run.result.data === 'object' &&
    run.result.data !== null &&
    'text' in run.result.data
  ) {
    console.log('Final content:', run.result.data.text);
  } else {
    console.error('Run failed:', run.result.error);
  }
}

createContentTeam().catch(console.error);
```

### Example 3: Agent with Memory

```typescript
import { agent, createDaemonClient } from '@animaOS-SWARM/sdk';

const client = createDaemonClient();

async function conversationalAgent() {
  const assistant = await client.agents.create(
    agent({
      name: 'assistant',
      provider: 'openai',
      model: 'gpt-4o-mini',
      system: 'You are a helpful assistant with memory of past conversations.',
      settings: {
        apiKey: process.env.OPENAI_API_KEY,
      },
    })
  );

  await client.agents.run(assistant.state.id, {
    text: 'My name is Alice and I love Python programming.',
  });

  const run = await client.agents.run(assistant.state.id, {
    text: 'What programming language do I like?',
  });

  if (
    run.result.status === 'success' &&
    typeof run.result.data === 'object' &&
    run.result.data !== null &&
    'text' in run.result.data
  ) {
    console.log('Response:', run.result.data.text);
  }

  const memories = await client.agents.recentMemories(assistant.state.id, {
    limit: 5,
  });
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

Check that your provider key is available to the daemon process or passed through the config `settings` you send.

### "Agent not found" Error

Verify the agent ID exists:

```typescript
const agents = await client.agents.list();
console.log(agents.map((agent) => agent.state.id));
```

---

## Additional Resources

- [Project README](../README.md) - Overview and quick start
- [Rust Core](../packages/core-rust/) - Reusable Rust runtime crates
- [Rust Daemon Host](../hosts/rust-daemon/) - Runnable Rust host, API surface, and operational commands
- [Core Package](../packages/core-ts/) - TypeScript core port with shared contracts and utilities
- [SDK Source](../packages/sdk/src/) - TypeScript daemon client implementation
