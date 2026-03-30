import { Command } from "commander"
import { createInterface } from "node:readline"
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

export const chatCommand = new Command("chat")
	.description("Interactive chat with an agent")
	.option("-m, --model <model>", "Model to use", "gpt-4o-mini")
	.option("-n, --name <name>", "Agent name", "task-agent")
	.option("--api-key <key>", "OpenAI API key (or set OPENAI_API_KEY env)")
	.action(async (opts: { model: string; name: string; apiKey?: string }) => {
		const apiKey = opts.apiKey ?? process.env.OPENAI_API_KEY
		if (!apiKey) {
			console.error("Error: OPENAI_API_KEY is required. Set it via --api-key or environment variable.")
			process.exit(1)
		}

		const bus = new EventBus()

		bus.on("tool:before", (e) => {
			const d = e.data as { toolName: string }
			process.stdout.write(`  [tool] ${d.toolName}...`)
		})
		bus.on("tool:after", (e) => {
			const d = e.data as { durationMs: number }
			console.log(` done (${d.durationMs}ms)`)
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

		console.log(`AnimaOS Kit — ${opts.name} (${opts.model})`)
		console.log('Type "exit" to quit.\n')

		const rl = createInterface({ input: process.stdin, output: process.stdout })

		const prompt = () => {
			rl.question("you > ", async (input) => {
				const trimmed = input.trim()
				if (!trimmed || trimmed === "exit") {
					console.log("Bye.")
					rl.close()
					return
				}

				const result = await runtime.run(trimmed)

				if (result.status === "success") {
					console.log(`\nagent > ${(result.data as { text: string })?.text}\n`)
				} else {
					console.log(`\n[error] ${result.error}\n`)
				}

				prompt()
			})
		}

		prompt()
	})
