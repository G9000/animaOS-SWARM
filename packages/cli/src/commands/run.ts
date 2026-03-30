import { Command } from "commander"
import {
	AgentRuntime,
	EventBus,
} from "@animaOS-SWARM/core"
import type { AgentConfig, IModelAdapter } from "@animaOS-SWARM/core"
import { SwarmCoordinator } from "@animaOS-SWARM/swarm"
import { allTools } from "../tools.js"
import { createAdapter } from "../agency/generator.js"

interface RunOptions {
	model: string
	provider: string
	name: string
	strategy?: string
	apiKey?: string
	tui: boolean
}

export const runCommand = new Command("run")
	.description("Run an agent or swarm with a task")
	.argument("<task>", "The task to execute")
	.option("-m, --model <model>", "Model to use", "gpt-4o-mini")
	.option("-p, --provider <provider>", "Provider: openai, anthropic, ollama", "openai")
	.option("-n, --name <name>", "Agent name (single-agent mode)", "task-agent")
	.option("-s, --strategy <strategy>", "Swarm strategy: supervisor, dynamic, round-robin")
	.option("--api-key <key>", "API key override")
	.option("--no-tui", "Disable TUI, use plain text output")
	.action(async (task: string, opts: RunOptions) => {
		const adapter = createAdapter(opts.provider, opts.apiKey)
		const bus = new EventBus()

		if (opts.strategy) {
			await runSwarm(task, opts.strategy, opts, adapter, bus)
		} else {
			await runSingleAgent(task, opts, adapter, bus)
		}
	})

async function runSwarm(
	task: string,
	strategy: string,
	opts: RunOptions,
	adapter: IModelAdapter,
	bus: EventBus,
): Promise<void> {
	const managerConfig: AgentConfig = {
		name: "manager",
		model: opts.model,
		system:
			"You are a task manager. Break complex tasks into subtasks and delegate to workers. Synthesize results into a final answer.",
		tools: allTools,
	}

	const workerConfig: AgentConfig = {
		name: "worker",
		model: opts.model,
		system:
			"You are a helpful worker agent. Complete the assigned task concisely and accurately.",
		tools: allTools,
	}

	const coordinator = new SwarmCoordinator(
		{
			strategy: strategy as "supervisor" | "dynamic" | "round-robin",
			manager: managerConfig,
			workers: [workerConfig],
		},
		adapter,
		bus,
	)

	if (opts.tui) {
		const { render } = await import("ink")
		const { default: React } = await import("react")
		const { App } = await import("@animaOS-SWARM/tui")

		const element = React.createElement(App, {
			eventBus: bus,
			strategy,
			task,
		})
		const instance = render(element)

		const result = await coordinator.run(task)
		// Wait for final events to flush through the TUI
		await new Promise((resolve) => setTimeout(resolve, 500))
		instance.unmount()

		if (result.status === "error") {
			process.exit(1)
		}
	} else {
		bus.on("agent:spawned", (e) => {
			const d = e.data as { agentId: string; name: string }
			console.log(`  [agent] spawned: ${d.name} (${d.agentId})`)
		})
		bus.on("tool:before", (e) => {
			const d = e.data as { toolName: string }
			console.log(`  [tool] calling: ${d.toolName}`)
		})

		console.log(`Swarm running with strategy "${strategy}" and model ${opts.model}...\n`)
		const result = await coordinator.run(task)

		console.log("\n--- Result ---")
		if (result.status === "success") {
			const data = result.data as { text?: string } | undefined
			console.log(data?.text ?? JSON.stringify(result.data))
		} else {
			console.error("Error:", result.error)
		}
		console.log(`\nDuration: ${result.durationMs}ms`)
	}
}

async function runSingleAgent(
	task: string,
	opts: RunOptions,
	adapter: IModelAdapter,
	bus: EventBus,
): Promise<void> {
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
			tools: allTools,
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
}
