# Swarm Command & Control — Terminal Experience

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One command launches a swarm of agents with a live TUI showing agents, messages, tools, and tokens in real-time.

**Architecture:** CLI `run` command creates a SwarmCoordinator in-process, wires EventBus events to an Ink (React-for-terminal) TUI. No separate server needed for v1 — TUI consumes events directly. The swarm runs in the background while the TUI renders.

**Tech Stack:** Ink (React terminal UI), Commander (CLI), existing core/swarm/tools packages.

---

## File Structure

### Fixes
- **Modify:** `packages/sdk/src/index.ts` — fix broken `allToolActions` import
- **Modify:** `packages/core/src/adapters/openai.ts` — add `generateStream`

### New: TUI Package (`packages/tui/`)
- `package.json` — package config with ink, react deps
- `tsconfig.json`, `tsconfig.lib.json` — TypeScript config
- `src/index.ts` — public exports
- `src/app.tsx` — main App component, orchestrates layout
- `src/types.ts` — TUI-specific types (SwarmEvent union, panel state)
- `src/hooks/use-event-log.ts` — hook to collect and manage events
- `src/components/agent-panel.tsx` — agent list with live status
- `src/components/message-stream.tsx` — agent-to-agent message feed
- `src/components/tool-panel.tsx` — tool calls in progress/completed
- `src/components/status-bar.tsx` — tokens, cost, elapsed time
- `src/components/header.tsx` — swarm name, strategy, agent count

### CLI Updates
- **Modify:** `packages/cli/package.json` — add tui + swarm deps
- **Modify:** `packages/cli/src/index.ts` — register new `run` flags
- **Modify:** `packages/cli/src/commands/run.ts` — swarm mode with TUI

---

## Task 1: Fix SDK Broken Import

**Files:**
- Modify: `packages/sdk/src/index.ts:83`

- [ ] **Step 1: Fix the import name**

In `packages/sdk/src/index.ts`, the import `allToolActions` does not exist in `@animaOS-SWARM/tools`. The correct export name is `ALL_TOOL_ACTIONS`.

```ts
// packages/sdk/src/index.ts — replace lines 73-84
// Tools
export {
	bashAction,
	readAction,
	writeAction,
	editAction,
	grepAction,
	globAction,
	listDirAction,
	multiEditAction,
	ALL_TOOL_ACTIONS,
} from "@animaOS-SWARM/tools"
```

- [ ] **Step 2: Verify build passes**

Run: `bun nx build sdk`
Expected: SUCCESS — no import errors.

- [ ] **Step 3: Commit**

```bash
git add packages/sdk/src/index.ts
git commit -m "fix(sdk): correct ALL_TOOL_ACTIONS import name"
```

---

## Task 2: Add OpenAI generateStream

**Files:**
- Modify: `packages/core/src/adapters/openai.ts`
- Test: `packages/core/src/adapters/openai-stream.spec.ts`

- [ ] **Step 1: Write the failing test**

Create `packages/core/src/adapters/openai-stream.spec.ts`:

```ts
import { describe, it, expect, vi } from "vitest"
import { OpenAIAdapter } from "./openai.js"

describe("OpenAIAdapter.generateStream", () => {
	it("should be defined", () => {
		const adapter = new OpenAIAdapter("fake-key")
		expect(adapter.generateStream).toBeDefined()
	})
})
```

- [ ] **Step 2: Run test to verify it fails**

Run: `bun nx test core`
Expected: FAIL — `generateStream` is not defined on `OpenAIAdapter`.

- [ ] **Step 3: Implement generateStream**

Add to `packages/core/src/adapters/openai.ts` after the `generate` method:

```ts
async *generateStream(
	config: ModelConfig,
	options: GenerateOptions,
): AsyncGenerator<StreamChunk> {
	const messages: OpenAI.Chat.Completions.ChatCompletionMessageParam[] = [
		{ role: "system", content: options.system },
	]

	for (const msg of options.messages) {
		if (msg.role === "assistant" && msg.content.metadata?.toolCalls) {
			const toolCalls = msg.content.metadata.toolCalls as ToolCall[]
			messages.push({
				role: "assistant",
				content: msg.content.text || null,
				tool_calls: toolCalls.map((tc) => ({
					id: tc.id,
					type: "function" as const,
					function: {
						name: tc.name,
						arguments: JSON.stringify(tc.args),
					},
				})),
			})
		} else if (msg.role === "tool") {
			const toolCallId = (msg.content.metadata?.toolCallId as string) ?? msg.id
			messages.push({
				role: "tool",
				tool_call_id: toolCallId,
				content: msg.content.text,
			})
		} else {
			messages.push({
				role: msg.role as "user" | "assistant",
				content: msg.content.text,
			})
		}
	}

	const tools = options.actions && options.actions.length > 0
		? actionsToTools(options.actions)
		: undefined

	const stream = await this.client.chat.completions.create({
		model: config.model,
		messages,
		tools,
		temperature: options.temperature ?? config.temperature,
		max_tokens: options.maxTokens ?? config.maxTokens,
		stream: true,
	})

	let currentToolCallId = ""
	let currentToolCallName = ""
	let currentToolCallArgs = ""

	for await (const chunk of stream) {
		const delta = chunk.choices[0]?.delta

		if (delta?.content) {
			yield { type: "text", content: delta.content }
		}

		if (delta?.tool_calls) {
			for (const tc of delta.tool_calls) {
				if (tc.id) {
					// New tool call starting
					if (currentToolCallId) {
						// Emit previous tool call
						let args: Record<string, unknown> = {}
						try { args = currentToolCallArgs ? JSON.parse(currentToolCallArgs) : {} } catch {}
						yield { type: "tool_call", toolCall: { id: currentToolCallId, name: currentToolCallName, args } }
					}
					currentToolCallId = tc.id
					currentToolCallName = tc.function?.name ?? ""
					currentToolCallArgs = tc.function?.arguments ?? ""
				} else {
					currentToolCallArgs += tc.function?.arguments ?? ""
				}
			}
		}

		if (chunk.choices[0]?.finish_reason) {
			// Emit any pending tool call
			if (currentToolCallId) {
				let args: Record<string, unknown> = {}
				try { args = currentToolCallArgs ? JSON.parse(currentToolCallArgs) : {} } catch {}
				yield { type: "tool_call", toolCall: { id: currentToolCallId, name: currentToolCallName, args } }
				currentToolCallId = ""
			}
			yield { type: "done" }
		}
	}
}
```

Import `StreamChunk` in the imports at top of file (add to existing import):

```ts
import type {
	IModelAdapter,
	ModelConfig,
	GenerateOptions,
	GenerateResult,
	ToolCall,
	StreamChunk,
	Action,
} from "../types/index.js"
```

- [ ] **Step 4: Run test to verify it passes**

Run: `bun nx test core`
Expected: ALL PASS.

- [ ] **Step 5: Commit**

```bash
git add packages/core/src/adapters/openai.ts packages/core/src/adapters/openai-stream.spec.ts
git commit -m "feat(core): add generateStream to OpenAI adapter"
```

---

## Task 3: Create TUI Package Scaffold

**Files:**
- Create: `packages/tui/package.json`
- Create: `packages/tui/tsconfig.json`
- Create: `packages/tui/tsconfig.lib.json`
- Create: `packages/tui/src/index.ts`
- Create: `packages/tui/src/types.ts`

- [ ] **Step 1: Create package.json**

```json
{
  "name": "@animaOS-SWARM/tui",
  "version": "0.0.1",
  "private": true,
  "type": "module",
  "main": "./dist/index.js",
  "types": "./dist/index.d.ts",
  "exports": {
    ".": {
      "import": "./dist/index.js",
      "types": "./dist/index.d.ts"
    }
  },
  "scripts": {
    "build": "tsc --build tsconfig.lib.json"
  },
  "dependencies": {
    "ink": "^5.2.0",
    "react": "^19.0.0",
    "@animaOS-SWARM/core": "workspace:*",
    "@animaOS-SWARM/swarm": "workspace:*"
  },
  "devDependencies": {
    "@types/react": "^19.0.0",
    "typescript": "~5.9.2"
  }
}
```

- [ ] **Step 2: Create tsconfig.json**

```json
{
  "extends": "../../tsconfig.base.json",
  "compilerOptions": {
    "jsx": "react-jsx",
    "outDir": "./dist",
    "rootDir": "./src"
  },
  "include": ["src/**/*.ts", "src/**/*.tsx"],
  "references": [
    { "path": "../core" },
    { "path": "../swarm" }
  ]
}
```

- [ ] **Step 3: Create tsconfig.lib.json**

```json
{
  "extends": "./tsconfig.json",
  "compilerOptions": {
    "declaration": true,
    "declarationMap": true,
    "outDir": "./dist",
    "rootDir": "./src"
  },
  "include": ["src/**/*.ts", "src/**/*.tsx"],
  "exclude": ["src/**/*.spec.ts", "src/**/*.test.ts"]
}
```

- [ ] **Step 4: Create src/types.ts**

```ts
import type { EventType, Event, TaskResult } from "@animaOS-SWARM/core"

/** Agent status as tracked by the TUI */
export type AgentDisplayStatus = "idle" | "thinking" | "running_tool" | "done" | "error"

/** An agent row in the agent panel */
export interface AgentEntry {
  id: string
  name: string
  status: AgentDisplayStatus
  tokens: number
  currentTool?: string
}

/** A message row in the message stream */
export interface MessageEntry {
  id: string
  from: string
  to: string
  content: string
  timestamp: number
}

/** A tool call row in the tool panel */
export interface ToolEntry {
  id: string
  agentId: string
  agentName: string
  toolName: string
  args: Record<string, unknown>
  status: "running" | "success" | "error"
  result?: string
  durationMs?: number
  timestamp: number
}

/** Aggregated swarm stats for the status bar */
export interface SwarmStats {
  totalTokens: number
  totalCost: number
  elapsed: number
  agentCount: number
  strategy: string
}
```

- [ ] **Step 5: Create src/index.ts**

```ts
export { App } from "./app.js"
export type {
  AgentEntry,
  AgentDisplayStatus,
  MessageEntry,
  ToolEntry,
  SwarmStats,
} from "./types.js"
```

- [ ] **Step 6: Install deps and verify build**

Run:
```bash
bun install
bun nx build tui
```
Expected: Build succeeds (will fail until App component exists — that's Task 5).

- [ ] **Step 7: Commit**

```bash
git add packages/tui/
git commit -m "feat(tui): scaffold TUI package with types"
```

---

## Task 4: TUI Event Hook

**Files:**
- Create: `packages/tui/src/hooks/use-event-log.ts`

This hook listens to the EventBus and maps raw events into the typed entries (AgentEntry, MessageEntry, ToolEntry, SwarmStats) that the TUI components consume.

- [ ] **Step 1: Create the hook**

```ts
import { useState, useEffect, useRef } from "react"
import type { IEventBus, Event } from "@animaOS-SWARM/core"
import type {
  AgentEntry,
  AgentDisplayStatus,
  MessageEntry,
  ToolEntry,
  SwarmStats,
} from "../types.js"

export interface UseEventLogOptions {
  eventBus: IEventBus
  strategy: string
}

export interface EventLogState {
  agents: AgentEntry[]
  messages: MessageEntry[]
  tools: ToolEntry[]
  stats: SwarmStats
  done: boolean
  result: string | null
  error: string | null
}

export function useEventLog({ eventBus, strategy }: UseEventLogOptions): EventLogState {
  const startTime = useRef(Date.now())
  const [agents, setAgents] = useState<AgentEntry[]>([])
  const [messages, setMessages] = useState<MessageEntry[]>([])
  const [tools, setTools] = useState<ToolEntry[]>([])
  const [done, setDone] = useState(false)
  const [result, setResult] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    const unsubs: Array<() => void> = []

    unsubs.push(eventBus.on("agent:spawned", (e: Event) => {
      const data = e.data as { agentId: string; name: string }
      setAgents((prev) => [
        ...prev,
        { id: data.agentId, name: data.name, status: "idle", tokens: 0 },
      ])
    }))

    unsubs.push(eventBus.on("task:started", (e: Event) => {
      const data = e.data as { agentId: string }
      setAgents((prev) =>
        prev.map((a) => (a.id === data.agentId ? { ...a, status: "thinking" as AgentDisplayStatus } : a)),
      )
    }))

    unsubs.push(eventBus.on("tool:before", (e: Event) => {
      const data = e.data as { agentId: string; toolName: string; args: Record<string, unknown> }
      setAgents((prev) =>
        prev.map((a) =>
          a.id === data.agentId
            ? { ...a, status: "running_tool" as AgentDisplayStatus, currentTool: data.toolName }
            : a,
        ),
      )
      setTools((prev) => [
        ...prev,
        {
          id: `${data.agentId}-${Date.now()}`,
          agentId: data.agentId,
          agentName: "",
          toolName: data.toolName,
          args: data.args,
          status: "running",
          timestamp: Date.now(),
        },
      ])
    }))

    unsubs.push(eventBus.on("tool:after", (e: Event) => {
      const data = e.data as { agentId: string; toolName: string; status: string; durationMs: number }
      setAgents((prev) =>
        prev.map((a) =>
          a.id === data.agentId
            ? { ...a, status: "thinking" as AgentDisplayStatus, currentTool: undefined }
            : a,
        ),
      )
      setTools((prev) => {
        const idx = prev.findLastIndex(
          (t) => t.agentId === data.agentId && t.toolName === data.toolName && t.status === "running",
        )
        if (idx < 0) return prev
        const updated = [...prev]
        updated[idx] = {
          ...updated[idx],
          status: data.status === "error" ? "error" : "success",
          durationMs: data.durationMs,
        }
        return updated
      })
    }))

    unsubs.push(eventBus.on("agent:message", (e: Event) => {
      const data = e.data as { from: string; to: string; message: { text: string } }
      setMessages((prev) => [
        ...prev,
        {
          id: `msg-${Date.now()}-${Math.random()}`,
          from: data.from,
          to: data.to,
          content: data.message.text.slice(0, 200),
          timestamp: Date.now(),
        },
      ])
    }))

    unsubs.push(eventBus.on("task:completed", (e: Event) => {
      const data = e.data as { agentId: string; result: { data?: { text?: string } } }
      setAgents((prev) =>
        prev.map((a) => (a.id === data.agentId ? { ...a, status: "done" as AgentDisplayStatus } : a)),
      )
    }))

    unsubs.push(eventBus.on("task:failed", (e: Event) => {
      const data = e.data as { agentId: string; error: string }
      setAgents((prev) =>
        prev.map((a) => (a.id === data.agentId ? { ...a, status: "error" as AgentDisplayStatus } : a)),
      )
    }))

    unsubs.push(eventBus.on("agent:terminated", (e: Event) => {
      const data = e.data as { agentId: string }
      setAgents((prev) =>
        prev.map((a) => (a.id === data.agentId ? { ...a, status: "done" as AgentDisplayStatus } : a)),
      )
    }))

    unsubs.push(eventBus.on("swarm:completed", (e: Event) => {
      const data = e.data as { result: { status: string; data?: { text?: string }; error?: string } }
      setDone(true)
      if (data.result.status === "success") {
        setResult(data.result.data?.text ?? "Done")
      } else {
        setError(data.result.error ?? "Unknown error")
      }
    }))

    return () => {
      for (const unsub of unsubs) unsub()
    }
  }, [eventBus])

  const totalTokens = agents.reduce((sum, a) => sum + a.tokens, 0)

  const stats: SwarmStats = {
    totalTokens,
    totalCost: totalTokens * 0.000003, // rough estimate, gpt-4o-mini
    elapsed: Date.now() - startTime.current,
    agentCount: agents.length,
    strategy,
  }

  return { agents, messages, tools, stats, done, result, error }
}
```

- [ ] **Step 2: Commit**

```bash
git add packages/tui/src/hooks/
git commit -m "feat(tui): add useEventLog hook for event consumption"
```

---

## Task 5: TUI Components

**Files:**
- Create: `packages/tui/src/components/header.tsx`
- Create: `packages/tui/src/components/agent-panel.tsx`
- Create: `packages/tui/src/components/message-stream.tsx`
- Create: `packages/tui/src/components/tool-panel.tsx`
- Create: `packages/tui/src/components/status-bar.tsx`
- Create: `packages/tui/src/app.tsx`

- [ ] **Step 1: Create header.tsx**

```tsx
import React from "react"
import { Box, Text } from "ink"

interface HeaderProps {
  strategy: string
  agentCount: number
  task: string
}

export function Header({ strategy, agentCount, task }: HeaderProps) {
  return (
    <Box borderStyle="single" paddingX={1}>
      <Text bold color="cyan">SWARM</Text>
      <Text> — </Text>
      <Text color="yellow">{strategy}</Text>
      <Text> — </Text>
      <Text>{agentCount} agents</Text>
      <Text> — </Text>
      <Text dimColor>{task.length > 60 ? task.slice(0, 57) + "..." : task}</Text>
    </Box>
  )
}
```

- [ ] **Step 2: Create agent-panel.tsx**

```tsx
import React from "react"
import { Box, Text } from "ink"
import type { AgentEntry } from "../types.js"

const STATUS_COLORS: Record<string, string> = {
  idle: "gray",
  thinking: "yellow",
  running_tool: "blue",
  done: "green",
  error: "red",
}

const STATUS_LABELS: Record<string, string> = {
  idle: "idle",
  thinking: "thinking...",
  running_tool: "running tool",
  done: "done",
  error: "error",
}

interface AgentPanelProps {
  agents: AgentEntry[]
}

export function AgentPanel({ agents }: AgentPanelProps) {
  if (agents.length === 0) {
    return (
      <Box paddingX={1}>
        <Text dimColor>Waiting for agents to spawn...</Text>
      </Box>
    )
  }

  return (
    <Box flexDirection="column" paddingX={1}>
      {agents.map((agent) => (
        <Box key={agent.id} gap={2}>
          <Text color="cyan">[{agent.name}]</Text>
          <Text color={STATUS_COLORS[agent.status] ?? "white"}>
            {STATUS_LABELS[agent.status] ?? agent.status}
            {agent.currentTool ? ` (${agent.currentTool})` : ""}
          </Text>
          <Text dimColor>tokens: {agent.tokens}</Text>
        </Box>
      ))}
    </Box>
  )
}
```

- [ ] **Step 3: Create message-stream.tsx**

```tsx
import React from "react"
import { Box, Text } from "ink"
import type { MessageEntry } from "../types.js"

interface MessageStreamProps {
  messages: MessageEntry[]
  maxVisible?: number
}

export function MessageStream({ messages, maxVisible = 10 }: MessageStreamProps) {
  const visible = messages.slice(-maxVisible)

  if (visible.length === 0) {
    return (
      <Box paddingX={1}>
        <Text dimColor>No messages yet</Text>
      </Box>
    )
  }

  return (
    <Box flexDirection="column" paddingX={1}>
      <Text bold dimColor>-- messages --</Text>
      {visible.map((msg) => (
        <Box key={msg.id}>
          <Text color="cyan">{msg.from.slice(0, 8)}</Text>
          <Text dimColor> → </Text>
          <Text color="green">{msg.to.slice(0, 8)}</Text>
          <Text>: {msg.content}</Text>
        </Box>
      ))}
    </Box>
  )
}
```

- [ ] **Step 4: Create tool-panel.tsx**

```tsx
import React from "react"
import { Box, Text } from "ink"
import type { ToolEntry } from "../types.js"

interface ToolPanelProps {
  tools: ToolEntry[]
  maxVisible?: number
}

export function ToolPanel({ tools, maxVisible = 5 }: ToolPanelProps) {
  const visible = tools.slice(-maxVisible)

  if (visible.length === 0) return null

  return (
    <Box flexDirection="column" paddingX={1}>
      <Text bold dimColor>-- tools --</Text>
      {visible.map((tool) => {
        const statusIcon = tool.status === "running" ? "..." : tool.status === "success" ? "ok" : "ERR"
        const argsPreview = JSON.stringify(tool.args).slice(0, 60)
        return (
          <Box key={tool.id}>
            <Text color={tool.status === "error" ? "red" : "gray"}>[{statusIcon}]</Text>
            <Text> </Text>
            <Text color="cyan">{tool.agentId.slice(0, 8)}</Text>
            <Text>: {tool.toolName}</Text>
            <Text dimColor>({argsPreview})</Text>
            {tool.durationMs !== undefined && <Text dimColor> {tool.durationMs}ms</Text>}
          </Box>
        )
      })}
    </Box>
  )
}
```

- [ ] **Step 5: Create status-bar.tsx**

```tsx
import React, { useState, useEffect } from "react"
import { Box, Text } from "ink"
import type { SwarmStats } from "../types.js"

interface StatusBarProps {
  stats: SwarmStats
  done: boolean
}

export function StatusBar({ stats, done }: StatusBarProps) {
  const [elapsed, setElapsed] = useState(0)

  useEffect(() => {
    if (done) return
    const interval = setInterval(() => {
      setElapsed((prev) => prev + 1)
    }, 1000)
    return () => clearInterval(interval)
  }, [done])

  return (
    <Box borderStyle="single" paddingX={1} justifyContent="space-between">
      <Text>tokens: <Text bold>{stats.totalTokens}</Text></Text>
      <Text>cost: <Text bold>${stats.totalCost.toFixed(4)}</Text></Text>
      <Text>elapsed: <Text bold>{elapsed}s</Text></Text>
      <Text>agents: <Text bold>{stats.agentCount}</Text></Text>
    </Box>
  )
}
```

- [ ] **Step 6: Create app.tsx**

```tsx
import React from "react"
import { Box, Text } from "ink"
import type { IEventBus } from "@animaOS-SWARM/core"
import { useEventLog } from "./hooks/use-event-log.js"
import { Header } from "./components/header.js"
import { AgentPanel } from "./components/agent-panel.js"
import { MessageStream } from "./components/message-stream.js"
import { ToolPanel } from "./components/tool-panel.js"
import { StatusBar } from "./components/status-bar.js"

interface AppProps {
  eventBus: IEventBus
  strategy: string
  task: string
}

export function App({ eventBus, strategy, task }: AppProps) {
  const { agents, messages, tools, stats, done, result, error } = useEventLog({
    eventBus,
    strategy,
  })

  return (
    <Box flexDirection="column">
      <Header strategy={strategy} agentCount={stats.agentCount} task={task} />
      <AgentPanel agents={agents} />
      <MessageStream messages={messages} />
      <ToolPanel tools={tools} />
      {done && result && (
        <Box paddingX={1} marginTop={1}>
          <Text bold color="green">Result: </Text>
          <Text>{result}</Text>
        </Box>
      )}
      {done && error && (
        <Box paddingX={1} marginTop={1}>
          <Text bold color="red">Error: </Text>
          <Text>{error}</Text>
        </Box>
      )}
      <StatusBar stats={stats} done={done} />
    </Box>
  )
}
```

- [ ] **Step 7: Update src/index.ts exports**

```ts
export { App } from "./app.js"
export { useEventLog } from "./hooks/use-event-log.js"
export { Header } from "./components/header.js"
export { AgentPanel } from "./components/agent-panel.js"
export { MessageStream } from "./components/message-stream.js"
export { ToolPanel } from "./components/tool-panel.js"
export { StatusBar } from "./components/status-bar.js"
export type {
  AgentEntry,
  AgentDisplayStatus,
  MessageEntry,
  ToolEntry,
  SwarmStats,
} from "./types.js"
```

- [ ] **Step 8: Install deps and verify build**

Run:
```bash
bun install
bun nx build tui
```
Expected: SUCCESS.

- [ ] **Step 9: Commit**

```bash
git add packages/tui/
git commit -m "feat(tui): add Ink TUI components for swarm visualization"
```

---

## Task 6: Wire CLI Run Command to Swarm + TUI

**Files:**
- Modify: `packages/cli/package.json`
- Modify: `packages/cli/src/commands/run.ts`
- Modify: `packages/cli/src/index.ts`

This is the task that produces the "wow" moment.

- [ ] **Step 1: Add dependencies to CLI package.json**

Add to `dependencies` in `packages/cli/package.json`:

```json
"@animaOS-SWARM/swarm": "workspace:*",
"@animaOS-SWARM/tui": "workspace:*",
"ink": "^5.2.0",
"react": "^19.0.0"
```

Add to `devDependencies`:

```json
"@types/react": "^19.0.0"
```

- [ ] **Step 2: Update tsconfig.json**

In `packages/cli/tsconfig.json`, add to the `references` array:

```json
{ "path": "../swarm" },
{ "path": "../tui" }
```

And add `"jsx": "react-jsx"` to `compilerOptions`.

- [ ] **Step 3: Rewrite run.ts with swarm + TUI support**

Replace `packages/cli/src/commands/run.ts`:

```ts
import { Command } from "commander"
import {
  AgentRuntime,
  EventBus,
  OpenAIAdapter,
  AnthropicAdapter,
  OllamaAdapter,
  action,
  type AgentConfig,
  type IModelAdapter,
} from "@animaOS-SWARM/core"
import { SwarmCoordinator } from "@animaOS-SWARM/swarm"

const builtinTools = [
  action({
    name: "get_current_time",
    description: "Get the current date and time",
    parameters: { type: "object", properties: {}, required: [] },
    handler: async () => ({
      status: "success" as const,
      data: new Date().toISOString(),
      durationMs: 0,
    }),
  }),
  action({
    name: "calculate",
    description: "Evaluate a math expression and return the result",
    parameters: {
      type: "object",
      properties: {
        expression: { type: "string", description: "The math expression to evaluate" },
      },
      required: ["expression"],
    },
    handler: async (_runtime, _message, args) => {
      try {
        const expr = args.expression as string
        const result = Function(`"use strict"; return (${expr})`)()
        return { status: "success" as const, data: String(result), durationMs: 0 }
      } catch (err) {
        return { status: "error" as const, error: String(err), durationMs: 0 }
      }
    },
  }),
]

function createAdapter(provider: string, apiKey?: string): IModelAdapter {
  switch (provider) {
    case "anthropic":
      return new AnthropicAdapter(apiKey ?? process.env.ANTHROPIC_API_KEY)
    case "ollama":
      return new OllamaAdapter(process.env.OLLAMA_BASE_URL)
    case "openai":
    default:
      return new OpenAIAdapter(apiKey ?? process.env.OPENAI_API_KEY)
  }
}

export const runCommand = new Command("run")
  .description("Run a task with a single agent or a swarm")
  .argument("<task>", "The task to execute")
  .option("-m, --model <model>", "Model to use", "gpt-4o-mini")
  .option("-p, --provider <provider>", "Model provider (openai, anthropic, ollama)", "openai")
  .option("-n, --name <name>", "Agent name", "task-agent")
  .option("-s, --strategy <strategy>", "Swarm strategy (supervisor, dynamic, round-robin)")
  .option("--api-key <key>", "API key (or set OPENAI_API_KEY / ANTHROPIC_API_KEY env)")
  .option("--no-tui", "Disable TUI, use plain text output")
  .action(async (task: string, opts) => {
    const adapter = createAdapter(opts.provider, opts.apiKey)
    const bus = new EventBus()

    // Swarm mode
    if (opts.strategy) {
      const manager: AgentConfig = {
        name: "manager",
        model: opts.model,
        system: "You are a task manager. Break complex tasks into subtasks and delegate to workers. Synthesize results into a final answer.",
        tools: builtinTools,
      }

      const worker: AgentConfig = {
        name: "worker",
        model: opts.model,
        system: "You are a helpful worker agent. Complete the assigned task concisely and accurately.",
        tools: builtinTools,
      }

      const coordinator = new SwarmCoordinator(
        {
          strategy: opts.strategy,
          manager,
          workers: [worker],
        },
        adapter,
        bus,
      )

      if (opts.tui !== false) {
        // Dynamic import to keep non-TUI path fast
        const { render } = await import("ink")
        const React = await import("react")
        const { App } = await import("@animaOS-SWARM/tui")

        const instance = render(
          React.createElement(App, { eventBus: bus, strategy: opts.strategy, task }),
        )

        const result = await coordinator.run(task)

        // Wait a beat for final events to render
        await new Promise((resolve) => setTimeout(resolve, 500))
        instance.unmount()

        if (result.status === "error") {
          process.exit(1)
        }
      } else {
        // Plain text fallback
        bus.on("agent:spawned", (e) => {
          const d = e.data as { name: string }
          console.log(`  [spawned] ${d.name}`)
        })
        bus.on("tool:before", (e) => {
          const d = e.data as { toolName: string }
          console.log(`  [tool] calling: ${d.toolName}`)
        })

        console.log(`Swarm (${opts.strategy}) running with ${opts.model}...\n`)
        const result = await coordinator.run(task)

        console.log("\n--- Result ---")
        if (result.status === "success") {
          console.log((result.data as { text: string })?.text)
        } else {
          console.error("Error:", result.error)
        }
        console.log(`\nDuration: ${result.durationMs}ms`)
      }
      return
    }

    // Single agent mode (existing behavior)
    bus.on("tool:before", (e) => {
      const d = e.data as { toolName: string }
      console.log(`  [tool] calling: ${d.toolName}`)
    })
    bus.on("tool:after", (e) => {
      const d = e.data as { toolName: string; durationMs: number }
      console.log(`  [tool] done: ${d.toolName} (${d.durationMs}ms)`)
    })

    const runtime = new AgentRuntime({
      config: {
        name: opts.name,
        model: opts.model,
        system: "You are a helpful task agent. Use tools when needed. Be concise.",
        tools: builtinTools,
      },
      modelAdapter: adapter,
      eventBus: bus,
    })

    console.log(`Agent "${opts.name}" running with ${opts.model}...\n`)
    const result = await runtime.run(task)

    console.log("\n--- Result ---")
    if (result.status === "success") {
      console.log((result.data as { text: string })?.text)
    } else {
      console.error("Error:", result.error)
    }
    console.log(`\nDuration: ${result.durationMs}ms | Tokens: ${runtime.getState().tokenUsage.totalTokens}`)
  })
```

- [ ] **Step 4: Run bun install and build**

```bash
bun install
bun nx build cli
```
Expected: SUCCESS.

- [ ] **Step 5: Test single agent mode still works**

```bash
node packages/cli/dist/index.js run "What is 2+2?" --no-tui
```
Expected: Agent runs, returns answer.

- [ ] **Step 6: Test swarm mode with TUI**

```bash
node packages/cli/dist/index.js run "What is the capital of France?" --strategy supervisor
```
Expected: TUI renders showing manager + worker agents, messages, tool calls, final result.

- [ ] **Step 7: Test swarm mode without TUI**

```bash
node packages/cli/dist/index.js run "What is 42 * 17?" --strategy supervisor --no-tui
```
Expected: Plain text output showing agent spawns, tool calls, result.

- [ ] **Step 8: Commit**

```bash
git add packages/cli/
git commit -m "feat(cli): add swarm mode with TUI visualization"
```

---

## Task 7: Add Provider Flag to Single Agent Mode

**Files:**
- Already done in Task 6 via `createAdapter` function.

- [ ] **Step 1: Test with Anthropic provider**

```bash
ANTHROPIC_API_KEY=sk-... node packages/cli/dist/index.js run "Hello" -p anthropic -m claude-sonnet-4-20250514
```

- [ ] **Step 2: Test with Ollama provider**

```bash
node packages/cli/dist/index.js run "Hello" -p ollama -m llama3
```

- [ ] **Step 3: Commit (if any fixes needed)**

---

## Task 8: Update Agent Token Tracking in TUI

The `useEventLog` hook doesn't currently track per-agent token usage because the events don't include it. We need to emit token info from the runtime.

**Files:**
- Modify: `packages/core/src/runtime/agent-runtime.ts`
- Modify: `packages/tui/src/hooks/use-event-log.ts`

- [ ] **Step 1: Emit token usage after each step**

In `packages/core/src/runtime/agent-runtime.ts`, add a new event emission at the end of the `step` method (after token tracking):

```ts
private async step(system: string, messages: Message[], actions: Action[]) {
  // ... existing code ...

  const result = await this.modelAdapter.generate(modelConfig, options)

  // Track token usage
  this.state.tokenUsage.promptTokens += result.usage.promptTokens
  this.state.tokenUsage.completionTokens += result.usage.completionTokens
  this.state.tokenUsage.totalTokens += result.usage.totalTokens

  // Emit token update
  await this.eventBus.emit("agent:tokens", {
    agentId: this.agentId,
    usage: { ...this.state.tokenUsage },
  }, this.agentId)

  return result
}
```

Add `"agent:tokens"` to the EventType union in `packages/core/src/types/events.ts`:

```ts
export type EventType =
  | "agent:spawned"
  | "agent:started"
  | "agent:completed"
  | "agent:failed"
  | "agent:terminated"
  | "agent:message"
  | "agent:tokens"
  | "task:started"
  | "task:completed"
  | "task:failed"
  | "tool:before"
  | "tool:after"
  | "swarm:created"
  | "swarm:completed"
```

- [ ] **Step 2: Consume token events in TUI hook**

Add to `useEventLog` in the `useEffect`:

```ts
unsubs.push(eventBus.on("agent:tokens", (e: Event) => {
  const data = e.data as { agentId: string; usage: { totalTokens: number } }
  setAgents((prev) =>
    prev.map((a) => (a.id === data.agentId ? { ...a, tokens: data.usage.totalTokens } : a)),
  )
}))
```

- [ ] **Step 3: Build and test**

```bash
bun nx build core tui cli
node packages/cli/dist/index.js run "What is 2+2?" --strategy supervisor
```
Expected: Agent panel shows live token counts per agent.

- [ ] **Step 4: Run existing tests**

```bash
bun nx test core
```
Expected: ALL PASS.

- [ ] **Step 5: Commit**

```bash
git add packages/core/ packages/tui/
git commit -m "feat(core,tui): add per-agent token tracking to TUI"
```

---

## Verification Checklist

After all tasks are complete:

- [ ] `bun nx build core swarm tools memory sdk tui cli` — all build
- [ ] `bun nx test core` — 9+ tests pass
- [ ] `bun nx test memory` — 12 tests pass
- [ ] Single agent mode: `node packages/cli/dist/index.js run "What is 2+2?"` — works
- [ ] Swarm TUI mode: `node packages/cli/dist/index.js run "Explain quantum computing" --strategy supervisor` — TUI renders with agents, messages, tools, tokens
- [ ] Swarm no-TUI mode: `node packages/cli/dist/index.js run "Hello" --strategy supervisor --no-tui` — plain text output
- [ ] Provider flag: `node packages/cli/dist/index.js run "Hello" -p anthropic` — uses Anthropic
