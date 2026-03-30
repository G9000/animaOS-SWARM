import { Command } from "commander"
import {
	AgentRuntime,
	EventBus,
	OpenAIAdapter,
	AnthropicAdapter,
	OllamaAdapter,
	action,
} from "@animaOS-SWARM/core"
import type { AgentConfig, IModelAdapter } from "@animaOS-SWARM/core"
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
		tools: builtinTools,
	}

	const workerConfig: AgentConfig = {
		name: "worker",
		model: opts.model,
		system:
			"You are a helpful worker agent. Complete the assigned task concisely and accurately.",
		tools: builtinTools,
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
}
