import { AgentRuntime, EventBus, OpenAIAdapter, agent, action } from "../packages/core-ts/src/index.js"

// Define a simple tool
const getCurrentTime = action({
	name: "get_current_time",
	description: "Get the current date and time",
	parameters: { type: "object", properties: {}, required: [] },
	handler: async () => ({
		status: "success" as const,
		data: new Date().toISOString(),
		durationMs: 0,
	}),
})

const calculate = action({
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
			// Simple safe eval for math
			const result = Function(`"use strict"; return (${expr})`)()
			return { status: "success" as const, data: String(result), durationMs: 0 }
		} catch (err) {
			return { status: "error" as const, error: String(err), durationMs: 0 }
		}
	},
})

// Create agent
const config = agent({
	name: "task-agent",
	model: "gpt-4o-mini",
	system: "You are a helpful task agent. Use tools when needed. Be concise.",
	tools: [getCurrentTime, calculate],
})

// Run it
const bus = new EventBus()

// Log events
bus.on("tool:before", (e) => console.log(`  [tool] calling: ${(e.data as any).toolName}`))
bus.on("tool:after", (e) => console.log(`  [tool] done: ${(e.data as any).toolName} (${(e.data as any).durationMs}ms)`))

const runtime = new AgentRuntime({
	config,
	modelAdapter: new OpenAIAdapter(),
	eventBus: bus,
})

console.log("Running agent...")
const result = await runtime.run("What time is it? Also, what is 42 * 17?")

console.log("\n--- Result ---")
console.log("Status:", result.status)
console.log("Response:", (result.data as any)?.text)
console.log("Duration:", result.durationMs, "ms")
console.log("Tokens:", runtime.getState().tokenUsage)
