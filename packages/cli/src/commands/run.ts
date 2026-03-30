import { Command } from "commander"
import { AgentRuntime, EventBus, OpenAIAdapter, action } from "@animaOS-SWARM/core"

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

export const runCommand = new Command("run")
	.description("Run an agent with a single task")
	.argument("<task>", "The task to execute")
	.option("-m, --model <model>", "Model to use", "gpt-4o-mini")
	.option("-n, --name <name>", "Agent name", "task-agent")
	.option("--api-key <key>", "OpenAI API key (or set OPENAI_API_KEY env)")
	.action(async (task: string, opts: { model: string; name: string; apiKey?: string }) => {
		const apiKey = opts.apiKey ?? process.env.OPENAI_API_KEY
		if (!apiKey) {
			console.error("Error: OPENAI_API_KEY is required. Set it via --api-key or environment variable.")
			process.exit(1)
		}

		const bus = new EventBus()

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
			modelAdapter: new OpenAIAdapter(apiKey),
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
