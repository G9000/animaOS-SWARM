# @animaOS-SWARM/core API Reference

This is the internal contributor reference for `packages/core-ts/`. For the public SDK API, see the docs site or [`packages/sdk`](../sdk).

`@animaOS-SWARM/core` provides shared TypeScript contracts used by the SDK, CLI, and any TypeScript host. It contains **no runtime dependencies** on HTTP frameworks or databases — it is pure types and lightweight helpers.

## Installation

```bash
bun add @animaOS-SWARM/core
```

## Usage

Import types for your own agent code, or use the helper functions for typed configuration:

```typescript
import type { AgentConfig, TaskResult, Action } from '@animaOS-SWARM/core';
import { agent, action, plugin } from '@animaOS-SWARM/core';
```

> **Note:** The `@animaOS-SWARM/sdk` re-exports everything from `@animaOS-SWARM/core`, so you typically only need one import source in application code.

## Agent Types

### `AgentConfig`

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | `string` | ✅ | Unique agent name |
| `model` | `string` | ✅ | LLM model identifier |
| `provider` | `string` | — | Provider key |
| `system` | `string` | — | System prompt |
| `bio` | `string` | — | Biography |
| `lore` | `string` | — | Background story |
| `knowledge` | `string[]` | — | Knowledge topics |
| `topics` | `string[]` | — | Conversation topics |
| `adjectives` | `string[]` | — | Personality descriptors |
| `style` | `string` | — | Response style |
| `tools` | `Action[]` | — | Available tools |
| `plugins` | `Plugin[]` | — | Registered plugins |
| `settings` | `AgentSettings` | — | Model hyperparameters |

### `AgentSettings`

| Field | Type | Description |
|-------|------|-------------|
| `temperature` | `number` | Sampling temperature |
| `maxTokens` | `number` | Max tokens per response |
| `timeout` | `number` | Request timeout (ms) |
| `maxRetries` | `number` | Retry count |
| `[key: string]` | `unknown` | Provider-specific params |

### `AgentState`

| Field | Type | Description |
|-------|------|-------------|
| `id` | `UUID` | Unique identifier |
| `name` | `string` | Agent name |
| `status` | `AgentStatus` | Runtime status |
| `config` | `AgentConfig` | Configuration snapshot |
| `createdAt` | `number` | Creation timestamp |
| `tokenUsage` | `TokenUsage` | Cumulative tokens |

### `AgentStatus`

`'idle' | 'running' | 'completed' | 'failed' | 'terminated'`

### `TokenUsage`

| Field | Type | Description |
|-------|------|-------------|
| `promptTokens` | `number` | Tokens sent to model |
| `completionTokens` | `number` | Tokens generated |
| `totalTokens` | `number` | Total consumed |

### `IAgentRuntime`

The runtime interface that actions, providers, and evaluators receive:

| Method | Signature | Description |
|--------|-----------|-------------|
| `run` | `(input: string \| Content) => Promise<TaskResult>` | Execute a task |
| `getActions` | `() => Action[]` | Get registered actions |
| `registerPlugin` | `(plugin: Plugin) => void` | Register a plugin |
| `send` | `(targetAgentId: string, message: Content) => Promise<void>` | Message another agent |
| `spawn` | `(config: AgentConfig & { task?: string }) => Promise<TaskResult>` | Spawn a child agent |
| `broadcast` | `(message: Content) => Promise<void>` | Broadcast to swarm |
| `stop` | `() => Promise<void>` | Stop the agent |

## Primitives

### `UUID`

Branded string type: `` `${string}-${string}-${string}-${string}-${string}` ``

### `Content`

| Field | Type | Description |
|-------|------|-------------|
| `text` | `string` | Primary text content |
| `attachments` | `Attachment[]` | Files, images, or URLs |
| `metadata` | `Record<string, unknown>` | Arbitrary metadata |

### `Attachment`

| Field | Type | Description |
|-------|------|-------------|
| `type` | `'file' \| 'image' \| 'url'` | Attachment kind |
| `name` | `string` | Display name |
| `data` | `string` | Base64 content or URL |

### `Message`

| Field | Type | Description |
|-------|------|-------------|
| `id` | `UUID` | Message ID |
| `agentId` | `UUID` | Sender agent |
| `roomId` | `UUID` | Room context |
| `content` | `Content` | Message body |
| `role` | `'user' \| 'assistant' \| 'system' \| 'tool'` | Message role |
| `createdAt` | `number` | Timestamp |

### `TaskResult<T>`

| Field | Type | Description |
|-------|------|-------------|
| `status` | `'success' \| 'error'` | Outcome |
| `data` | `T` | Success payload |
| `error` | `string` | Error message |
| `durationMs` | `number` | Execution time |

## Components

### `Action`

The primary extension primitive — a tool an agent can invoke.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | `string` | ✅ | Unique identifier |
| `description` | `string` | ✅ | LLM-visible description |
| `parameters` | `Record<string, unknown>` | ✅ | JSON Schema parameters |
| `handler` | `(runtime, message, args) => Promise<TaskResult>` | ✅ | Execution logic |
| `validate` | `(runtime, message) => Promise<boolean>` | — | Availability check |
| `examples` | `ActionExample[]` | — | Few-shot examples |

### `ActionExample`

| Field | Type |
|-------|------|
| `input` | `string` |
| `args` | `Record<string, unknown>` |
| `output` | `string` |

### `Provider`

Dynamic context injected before each LLM call.

| Field | Type | Required |
|-------|------|----------|
| `name` | `string` | ✅ |
| `description` | `string` | ✅ |
| `get` | `(runtime, message) => Promise<ProviderResult>` | ✅ |

### `ProviderResult`

| Field | Type |
|-------|------|
| `text` | `string` |
| `metadata` | `Record<string, unknown>` |

### `Evaluator`

Post-processing logic after each agent response.

| Field | Type | Required |
|-------|------|----------|
| `name` | `string` | ✅ |
| `description` | `string` | ✅ |
| `validate` | `(runtime, message) => Promise<boolean>` | ✅ |
| `handler` | `(runtime, message, response) => Promise<EvaluatorResult>` | ✅ |

### `EvaluatorResult`

| Field | Type |
|-------|------|
| `score` | `number` |
| `feedback` | `string` |
| `followUp` | `Content` |
| `metadata` | `Record<string, unknown>` |

## Plugins

### `Plugin`

Bundles actions, providers, and evaluators into a reusable module.

| Field | Type | Description |
|-------|------|-------------|
| `name` | `string` | Plugin identifier |
| `description` | `string` | Human-readable description |
| `actions` | `Action[]` | Tools |
| `providers` | `Provider[]` | Context providers |
| `evaluators` | `Evaluator[]` | Post-response evaluators |
| `init` | `(runtime) => Promise<void>` | Registration hook |
| `cleanup` | `(runtime) => Promise<void>` | Shutdown hook |

## Model Types

### `ModelProvider`

`'openai' | 'anthropic' | 'ollama' | 'openrouter' | string`

### `ModelConfig`

| Field | Type | Description |
|-------|------|-------------|
| `provider` | `ModelProvider` | Provider key |
| `model` | `string` | Model identifier |
| `apiKey` | `string` | API key (optional if set via env) |
| `baseUrl` | `string` | Custom base URL |
| `temperature` | `number` | Sampling temperature |
| `maxTokens` | `number` | Max tokens |

### `GenerateOptions`

| Field | Type | Description |
|-------|------|-------------|
| `system` | `string` | System prompt |
| `messages` | `Message[]` | Conversation history |
| `actions` | `Action[]` | Available actions |
| `temperature` | `number` | Override temperature |
| `maxTokens` | `number` | Override max tokens |

### `GenerateResult`

| Field | Type | Description |
|-------|------|-------------|
| `content` | `Content` | Generated response |
| `toolCalls` | `ToolCall[]` | Invoked tools |
| `usage` | `{ promptTokens, completionTokens, totalTokens }` | Token usage |
| `stopReason` | `'end' \| 'tool_call' \| 'max_tokens'` | Why generation stopped |

### `ToolCall`

| Field | Type |
|-------|------|
| `id` | `string` |
| `name` | `string` |
| `args` | `Record<string, unknown>` |

### `StreamChunk`

| Field | Type | Description |
|-------|------|-------------|
| `type` | `'text' \| 'tool_call' \| 'done'` | Chunk kind |
| `content` | `string` | Text delta |
| `toolCall` | `ToolCall` | Tool call data |

## Helpers

The core exports typed identity functions that provide full IntelliSense:

```typescript
import { agent, action, plugin } from '@animaOS-SWARM/core';

const config = agent({
  name: 'my-agent',
  model: 'gpt-4o',
  // TypeScript knows every valid field here
});
```

| Function | Signature |
|----------|-----------|
| `agent(config)` | `<T extends AgentConfig>(config: T) => T` |
| `action(config)` | `<T extends Action>(config: T) => T` |
| `plugin(config)` | `<T extends Plugin>(config: T) => T` |

## Events

### `Event<T>`

| Field | Type |
|-------|------|
| `id` | `UUID` |
| `type` | `EventType` |
| `data` | `T` |
| `createdAt` | `number` |

### `EventType`

`'message' | 'action' | 'evaluation' | 'thought' | 'goal' | 'memory' | 'agent' | 'swarm' | 'system'`

### `IEventBus`

| Method | Signature |
|--------|-----------|
| `on` | `(type: EventType, handler: EventHandler<T>) => void` |
| `off` | `(type: EventType, handler: EventHandler<T>) => void` |
| `emit` | `(event: Event<T>) => void` |

## Daemon Health

### `DaemonWarningSource`

`'manual' | 'poll'`

### `DaemonWarningTransition`

| Field | Type | Description |
|-------|------|-------------|
| `from` | `string \| null` | Previous warning state |
| `to` | `string \| null` | Current warning state |
| `source` | `DaemonWarningSource` | What triggered the change |

## Consumers

`@animaOS-SWARM/core` is consumed by:

| Package | Purpose |
|---------|---------|
| `@animaOS-SWARM/sdk` | HTTP client for the daemon |
| `@animaOS-SWARM/cli` | Command-line interface |
| `@animaOS-SWARM/memory` | Memory system types |
| `@animaOS-SWARM/swarm` | Swarm coordination types |
