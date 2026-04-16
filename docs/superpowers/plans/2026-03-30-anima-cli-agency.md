# Anima CLI — Agency Creation & Launch

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `anima create` scaffolds an agency (orchestrator defines its team via LLM), `anima launch` runs it with TUI, `anima run` does quick one-off tasks.

**Architecture:** The `create` command uses @clack/prompts for interactive input, calls the LLM to generate worker agent definitions based on the orchestrator's purpose, saves everything as YAML config files. The `launch` command reads the config and runs a SwarmCoordinator with TUI. AgentConfig gets a `bio` field for personality.

**Tech Stack:** @clack/prompts (interactive CLI), js-yaml (YAML parsing), existing core/swarm/tui packages.

---

## File Structure

### Core Type Update
- **Modify:** `packages/core/src/types/agent.ts` — add `bio` field to AgentConfig

### Agency Config Types & Loader
- **Create:** `packages/cli/src/agency/types.ts` — AgencyConfig, AgentDefinition types
- **Create:** `packages/cli/src/agency/loader.ts` — load and validate agency YAML files
- **Create:** `packages/cli/src/agency/generator.ts` — LLM-powered agent team generator

### CLI Commands
- **Modify:** `packages/cli/src/index.ts` — rename to `anima`, add create + launch commands
- **Modify:** `packages/cli/package.json` — rename bin, add deps (js-yaml, @clack/prompts)
- **Create:** `packages/cli/src/commands/create.ts` — interactive agency builder
- **Create:** `packages/cli/src/commands/launch.ts` — run agency from config with TUI
- **Modify:** `packages/cli/src/commands/run.ts` — inject bio into system prompts

### Runtime Update
- **Modify:** `packages/core/src/runtime/agent-runtime.ts` — prepend bio to system prompt

---

## Task 1: Add `bio` to AgentConfig

**Files:**
- Modify: `packages/core/src/types/agent.ts`
- Modify: `packages/core/src/runtime/agent-runtime.ts`

- [ ] **Step 1: Add bio field to AgentConfig**

In `packages/core/src/types/agent.ts`, add `bio` after `model`:

```ts
export interface AgentConfig {
	name: string
	model: string
	bio?: string
	provider?: string
	system?: string
	tools?: Action[]
	plugins?: Plugin[]
	settings?: AgentSettings
}
```

- [ ] **Step 2: Prepend bio to system prompt in agent runtime**

In `packages/core/src/runtime/agent-runtime.ts`, find the `run` method where it builds `systemParts`. Change:

```ts
const systemParts = [this.config.system ?? "You are a helpful task agent."]
```

To:

```ts
const systemParts: string[] = []
if (this.config.bio) {
	systemParts.push(`## Who You Are\n${this.config.bio}`)
}
systemParts.push(this.config.system ?? "You are a helpful task agent.")
```

- [ ] **Step 3: Verify tests pass**

Run: `bun nx test core`
Expected: ALL PASS (10 tests).

- [ ] **Step 4: Verify build**

Run: `bun nx build core`
Expected: SUCCESS.

---

## Task 2: Agency Config Types & YAML Loader

**Files:**
- Create: `packages/cli/src/agency/types.ts`
- Create: `packages/cli/src/agency/loader.ts`

- [ ] **Step 1: Create agency types**

Create `packages/cli/src/agency/types.ts`:

```ts
export interface AgentDefinition {
	name: string
	bio: string
	system: string
	model?: string
	tools?: string[]
}

export interface AgencyConfig {
	name: string
	description: string
	model: string
	provider: string
	strategy: "supervisor" | "dynamic" | "round-robin"
	orchestrator: AgentDefinition
	agents: AgentDefinition[]
}
```

- [ ] **Step 2: Create YAML loader**

Add `js-yaml` and `@types/js-yaml` to `packages/cli/package.json` dependencies:

```json
"js-yaml": "^4.1.0"
```

And devDependencies:

```json
"@types/js-yaml": "^4.0.9"
```

Create `packages/cli/src/agency/loader.ts`:

```ts
import { readFileSync, existsSync } from "node:fs"
import { join } from "node:path"
import yaml from "js-yaml"
import type { AgencyConfig } from "./types.js"

export function loadAgency(dir: string): AgencyConfig {
	const configPath = join(dir, "anima.yaml")

	if (!existsSync(configPath)) {
		throw new Error(`No anima.yaml found in ${dir}`)
	}

	const raw = readFileSync(configPath, "utf-8")
	const parsed = yaml.load(raw) as AgencyConfig

	if (!parsed.name) throw new Error("Agency config missing 'name'")
	if (!parsed.orchestrator) throw new Error("Agency config missing 'orchestrator'")
	if (!parsed.agents || parsed.agents.length === 0) {
		throw new Error("Agency config missing 'agents'")
	}

	return {
		name: parsed.name,
		description: parsed.description ?? "",
		model: parsed.model ?? "gpt-4o-mini",
		provider: parsed.provider ?? "openai",
		strategy: parsed.strategy ?? "supervisor",
		orchestrator: {
			name: parsed.orchestrator.name ?? "orchestrator",
			bio: parsed.orchestrator.bio ?? "",
			system: parsed.orchestrator.system ?? "",
			model: parsed.orchestrator.model,
			tools: parsed.orchestrator.tools,
		},
		agents: parsed.agents.map((a) => ({
			name: a.name ?? "agent",
			bio: a.bio ?? "",
			system: a.system ?? "",
			model: a.model,
			tools: a.tools,
		})),
	}
}

export function agencyExists(dir: string): boolean {
	return existsSync(join(dir, "anima.yaml"))
}
```

- [ ] **Step 3: Install deps and verify build**

Run: `bun install && bun nx build cli`
Expected: SUCCESS.

---

## Task 3: LLM Agent Team Generator

**Files:**
- Create: `packages/cli/src/agency/generator.ts`

This is the core of `anima create` — the LLM generates worker agents based on the agency purpose.

- [ ] **Step 1: Create generator**

Create `packages/cli/src/agency/generator.ts`:

```ts
import {
	OpenAIAdapter,
	AnthropicAdapter,
	OllamaAdapter,
	type IModelAdapter,
	type ModelConfig,
} from "@animaOS-SWARM/core"
import type { AgentDefinition } from "./types.js"

export function createAdapter(provider: string, apiKey?: string): IModelAdapter {
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

export async function generateAgentTeam(opts: {
	agencyName: string
	agencyDescription: string
	orchestratorBio: string
	model: string
	provider: string
	apiKey?: string
}): Promise<AgentDefinition[]> {
	const adapter = createAdapter(opts.provider, opts.apiKey)

	const config: ModelConfig = {
		provider: opts.provider,
		model: opts.model,
	}

	const result = await adapter.generate(config, {
		system: `You are an agency architect. Given an agency's purpose and orchestrator description, suggest the ideal team of worker agents.

Respond with ONLY valid JSON — an array of agent objects. No markdown, no explanation. Each agent:
{
  "name": "short-lowercase-name",
  "bio": "1-2 sentence description of who this agent is and what makes them good at their role",
  "system": "System prompt instruction for this agent"
}

Create 2-4 agents. Each should have a distinct role that contributes to the agency's purpose. Make bios feel like real team members, not generic descriptions.`,
		messages: [
			{
				id: "00000000-0000-0000-0000-000000000000" as `${string}-${string}-${string}-${string}-${string}`,
				agentId: "00000000-0000-0000-0000-000000000000" as `${string}-${string}-${string}-${string}-${string}`,
				roomId: "00000000-0000-0000-0000-000000000000" as `${string}-${string}-${string}-${string}-${string}`,
				content: {
					text: `Agency: ${opts.agencyName}\nPurpose: ${opts.agencyDescription}\nOrchestrator: ${opts.orchestratorBio}`,
				},
				role: "user",
				createdAt: Date.now(),
			},
		],
	})

	const text = result.content.text.trim()

	// Parse JSON from response (handle markdown code blocks)
	const jsonStr = text.startsWith("[") ? text : text.replace(/```json?\n?/g, "").replace(/```/g, "").trim()

	try {
		const agents = JSON.parse(jsonStr) as AgentDefinition[]
		return agents.map((a) => ({
			name: a.name,
			bio: a.bio,
			system: a.system,
			tools: a.tools,
		}))
	} catch {
		throw new Error(`Failed to parse agent suggestions from LLM:\n${text}`)
	}
}
```

- [ ] **Step 2: Verify build**

Run: `bun nx build cli`
Expected: SUCCESS.

---

## Task 4: `anima create` Command

**Files:**
- Create: `packages/cli/src/commands/create.ts`
- Modify: `packages/cli/package.json` — add @clack/prompts dep

- [ ] **Step 1: Add @clack/prompts dependency**

Add to `packages/cli/package.json` dependencies:

```json
"@clack/prompts": "^0.10.0",
"picocolors": "^1.1.0"
```

- [ ] **Step 2: Create the create command**

Create `packages/cli/src/commands/create.ts`:

```ts
import { Command } from "commander"
import * as clack from "@clack/prompts"
import pc from "picocolors"
import { writeFileSync, mkdirSync, existsSync } from "node:fs"
import { join } from "node:path"
import yaml from "js-yaml"
import { generateAgentTeam } from "../agency/generator.js"
import type { AgencyConfig, AgentDefinition } from "../agency/types.js"

export const createCommand = new Command("create")
	.description("Create a new agent agency")
	.argument("[name]", "Agency name")
	.option("-p, --provider <provider>", "Model provider", "openai")
	.option("-m, --model <model>", "Model to use", "gpt-4o-mini")
	.option("--api-key <key>", "API key")
	.action(async (name: string | undefined, opts) => {
		clack.intro(pc.bgCyan(pc.black(" anima ")))

		// Step 1: Agency name
		let agencyName = name
		if (!agencyName) {
			const input = await clack.text({
				message: "Agency name:",
				placeholder: "my-agency",
				validate: (v) => {
					if (!v.trim()) return "Name is required"
					if (existsSync(v)) return `Directory '${v}' already exists`
					return undefined
				},
			})
			if (clack.isCancel(input)) { clack.cancel("Cancelled."); process.exit(0) }
			agencyName = input as string
		}

		// Step 2: What does this agency do?
		const description = await clack.text({
			message: "What does this agency do?",
			placeholder: "Creates high-quality blog posts and articles",
			validate: (v) => (!v.trim() ? "Description is required" : undefined),
		})
		if (clack.isCancel(description)) { clack.cancel("Cancelled."); process.exit(0) }

		// Step 3: Orchestrator bio
		const orchestratorBio = await clack.text({
			message: "Describe the orchestrator (the boss agent):",
			placeholder: "Experienced content director who plans, delegates, and reviews work",
			validate: (v) => (!v.trim() ? "Bio is required" : undefined),
		})
		if (clack.isCancel(orchestratorBio)) { clack.cancel("Cancelled."); process.exit(0) }

		// Step 4: Generate agent team
		const spinner = clack.spinner()
		spinner.start("Building your team...")

		let suggestedAgents: AgentDefinition[]
		try {
			suggestedAgents = await generateAgentTeam({
				agencyName: agencyName!,
				agencyDescription: description as string,
				orchestratorBio: orchestratorBio as string,
				model: opts.model,
				provider: opts.provider,
				apiKey: opts.apiKey,
			})
			spinner.stop(`Generated ${suggestedAgents.length} agents`)
		} catch (err) {
			spinner.stop("Failed to generate agents")
			clack.log.error(err instanceof Error ? err.message : String(err))
			process.exit(1)
		}

		// Step 5: Show suggestions
		clack.log.info(pc.bold("Suggested team:"))
		for (const agent of suggestedAgents) {
			clack.log.message(`  ${pc.cyan(`[${agent.name}]`)} — ${agent.bio}`)
		}

		const accepted = await clack.confirm({
			message: "Accept this team?",
		})
		if (clack.isCancel(accepted) || !accepted) {
			clack.cancel("Cancelled.")
			process.exit(0)
		}

		// Step 6: Scaffold project
		const agencyDir = agencyName!
		mkdirSync(agencyDir, { recursive: true })

		const config: AgencyConfig = {
			name: agencyName!,
			description: description as string,
			model: opts.model,
			provider: opts.provider,
			strategy: "supervisor",
			orchestrator: {
				name: "orchestrator",
				bio: orchestratorBio as string,
				system: `You are the orchestrator of the ${agencyName} agency. ${description}. Break tasks into subtasks and delegate to your team. Synthesize results into a final deliverable.`,
			},
			agents: suggestedAgents,
		}

		const yamlStr = yaml.dump(config, { lineWidth: 120, noRefs: true })
		writeFileSync(join(agencyDir, "anima.yaml"), yamlStr)

		clack.log.success(`Agency saved to ${pc.cyan(agencyDir + "/anima.yaml")}`)

		clack.outro(`Run with: ${pc.cyan(`cd ${agencyDir} && anima launch "your task here"`)}`)
	})
```

- [ ] **Step 3: Install deps and verify build**

Run: `bun install && bun nx build cli`
Expected: SUCCESS.

---

## Task 5: `anima launch` Command

**Files:**
- Create: `packages/cli/src/commands/launch.ts`

- [ ] **Step 1: Create launch command**

Create `packages/cli/src/commands/launch.ts`:

```ts
import { Command } from "commander"
import {
	EventBus,
	action,
	type AgentConfig,
} from "@animaOS-SWARM/core"
import { SwarmCoordinator } from "@animaOS-SWARM/swarm"
import { loadAgency, agencyExists } from "../agency/loader.js"
import { createAdapter } from "../agency/generator.js"

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

export const launchCommand = new Command("launch")
	.description("Launch an agency from the current directory")
	.argument("<task>", "The task for the agency")
	.option("--dir <dir>", "Agency directory", ".")
	.option("--api-key <key>", "API key override")
	.option("--no-tui", "Disable TUI, use plain text output")
	.action(async (task: string, opts) => {
		const dir = opts.dir as string

		if (!agencyExists(dir)) {
			console.error(`No anima.yaml found in ${dir}. Run 'anima create' first.`)
			process.exit(1)
		}

		const agency = loadAgency(dir)
		const adapter = createAdapter(agency.provider, opts.apiKey)
		const bus = new EventBus()

		// Build orchestrator AgentConfig
		const managerConfig: AgentConfig = {
			name: agency.orchestrator.name,
			model: agency.orchestrator.model ?? agency.model,
			bio: agency.orchestrator.bio,
			system: agency.orchestrator.system,
			tools: builtinTools,
		}

		// Build worker AgentConfigs
		const workerConfigs: AgentConfig[] = agency.agents.map((a) => ({
			name: a.name,
			model: a.model ?? agency.model,
			bio: a.bio,
			system: a.system,
			tools: builtinTools,
		}))

		const coordinator = new SwarmCoordinator(
			{
				strategy: agency.strategy,
				manager: managerConfig,
				workers: workerConfigs,
			},
			adapter,
			bus,
		)

		if (opts.tui !== false) {
			const { render } = await import("ink")
			const React = await import("react")
			const { App } = await import("@animaOS-SWARM/tui")

			const instance = render(
				React.createElement(App, { eventBus: bus, strategy: agency.strategy, task }),
			)

			const result = await coordinator.run(task)
			await new Promise((resolve) => setTimeout(resolve, 500))
			instance.unmount()

			if (result.status === "error") process.exit(1)
		} else {
			bus.on("agent:spawned", (e) => {
				const d = e.data as { name: string }
				console.log(`  [spawned] ${d.name}`)
			})
			bus.on("tool:before", (e) => {
				const d = e.data as { toolName: string }
				console.log(`  [tool] ${d.toolName}`)
			})

			console.log(`Launching ${agency.name} (${agency.strategy})...\n`)
			const result = await coordinator.run(task)

			console.log("\n--- Result ---")
			if (result.status === "success") {
				console.log((result.data as { text: string })?.text)
			} else {
				console.error("Error:", result.error)
			}
		}
	})
```

- [ ] **Step 2: Verify build**

Run: `bun nx build cli`
Expected: SUCCESS.

---

## Task 6: Wire Up CLI Entry Point

**Files:**
- Modify: `packages/cli/src/index.ts`
- Modify: `packages/cli/package.json`

- [ ] **Step 1: Rename binary in package.json**

In `packages/cli/package.json`, change the `bin` field:

```json
"bin": {
	"anima": "./dist/index.js"
}
```

- [ ] **Step 2: Update CLI entry point**

Replace `packages/cli/src/index.ts`:

```ts
#!/usr/bin/env node
import { Command } from "commander"
import { runCommand } from "./commands/run.js"
import { chatCommand } from "./commands/chat.js"
import { createCommand } from "./commands/create.js"
import { launchCommand } from "./commands/launch.js"

const program = new Command()

program
	.name("anima")
	.description("animaOS-SWARM — Command & control your AI agent swarms")
	.version("0.0.1")

program.addCommand(createCommand)
program.addCommand(launchCommand)
program.addCommand(runCommand)
program.addCommand(chatCommand)

program.parse()
```

- [ ] **Step 3: Build and verify**

Run: `bun install && bun nx build cli`
Expected: SUCCESS.

- [ ] **Step 4: Test help output**

Run: `node packages/cli/dist/index.js --help`
Expected output shows `anima` with create, launch, run, chat commands.

---

## Task 7: End-to-End Test

No automated test — manual verification of the full flow.

- [ ] **Step 1: Test create**

```bash
node packages/cli/dist/index.js create test-agency
```

Expected: Interactive prompts → LLM generates agents → saves `test-agency/anima.yaml`.

- [ ] **Step 2: Inspect generated config**

```bash
cat test-agency/anima.yaml
```

Expected: Valid YAML with orchestrator + 2-4 worker agents, each with name, bio, system.

- [ ] **Step 3: Test launch**

```bash
node packages/cli/dist/index.js launch "Write a haiku about coding" --dir test-agency
```

Expected: TUI renders showing orchestrator + workers coordinating.

- [ ] **Step 4: Test run still works**

```bash
node packages/cli/dist/index.js run "What is 2+2?"
```

Expected: Single agent mode still works.

- [ ] **Step 5: Cleanup**

```bash
rm -rf test-agency
```

---

## Verification Checklist

- [ ] `bun nx build cli` — builds successfully
- [ ] `bun nx test core` — 10 tests pass
- [ ] `anima --help` — shows create, launch, run, chat
- [ ] `anima create` — interactive agency builder works
- [ ] `anima launch "task" --dir ./agency` — runs swarm from config
- [ ] `anima run "task"` — single agent quick task still works
- [ ] Agent bios appear in system prompts
