import { describe, it, expect, vi } from "vitest"
import { AgentRuntime } from "./agent-runtime.js"
import { EventBus } from "./event-bus.js"
import type { IModelAdapter, GenerateResult, Action } from "../types/index.js"

function mockModelAdapter(response: string): IModelAdapter {
	return {
		provider: "test",
		generate: vi.fn().mockResolvedValue({
			content: { text: response },
			toolCalls: undefined,
			usage: { promptTokens: 10, completionTokens: 5, totalTokens: 15 },
			stopReason: "end",
		} satisfies GenerateResult),
	}
}

function mockToolAction(name: string, result: string): Action {
	return {
		name,
		description: `Test tool ${name}`,
		parameters: {},
		handler: vi.fn().mockResolvedValue({
			status: "success" as const,
			data: result,
			durationMs: 1,
		}),
	}
}

describe("AgentRuntime", () => {
	it("should run a simple task", async () => {
		const adapter = mockModelAdapter("Hello, world!")
		const bus = new EventBus()

		const runtime = new AgentRuntime({
			config: { name: "test-agent", model: "test-model" },
			modelAdapter: adapter,
			eventBus: bus,
		})

		const result = await runtime.run("Say hello")

		expect(result.status).toBe("success")
		expect(result.data).toEqual({ text: "Hello, world!" })
	})

	it("should execute tool calls in the loop", async () => {
		const tool = mockToolAction("search", "found results")
		const adapter: IModelAdapter = {
			provider: "test",
			generate: vi.fn()
				.mockResolvedValueOnce({
					content: { text: "" },
					toolCalls: [{ id: "1", name: "search", args: { query: "test" } }],
					usage: { promptTokens: 10, completionTokens: 5, totalTokens: 15 },
					stopReason: "tool_call",
				})
				.mockResolvedValueOnce({
					content: { text: "Here are the results." },
					toolCalls: undefined,
					usage: { promptTokens: 20, completionTokens: 10, totalTokens: 30 },
					stopReason: "end",
				}),
		}

		const runtime = new AgentRuntime({
			config: { name: "test-agent", model: "test-model", tools: [tool] },
			modelAdapter: adapter,
			eventBus: new EventBus(),
		})

		const result = await runtime.run("Search for something")

		expect(result.status).toBe("success")
		expect(tool.handler).toHaveBeenCalledOnce()
	})

	it("should register plugins", () => {
		const runtime = new AgentRuntime({
			config: { name: "test", model: "test" },
			modelAdapter: mockModelAdapter("ok"),
			eventBus: new EventBus(),
		})

		runtime.registerPlugin({
			name: "test-plugin",
			description: "A test plugin",
			actions: [mockToolAction("my-action", "result")],
		})

		expect(runtime.getActions()).toHaveLength(1)
		expect(runtime.getActions()[0].name).toBe("my-action")
	})

	it("should track token usage", async () => {
		const runtime = new AgentRuntime({
			config: { name: "test", model: "test" },
			modelAdapter: mockModelAdapter("ok"),
			eventBus: new EventBus(),
		})

		await runtime.run("test")
		const state = runtime.getState()

		expect(state.tokenUsage.totalTokens).toBe(15)
	})

	it("should throw when sending without coordinator", async () => {
		const runtime = new AgentRuntime({
			config: { name: "test", model: "test" },
			modelAdapter: mockModelAdapter("ok"),
			eventBus: new EventBus(),
		})

		await expect(runtime.send("other", { text: "hi" }))
			.rejects.toThrow("not connected to a swarm coordinator")
	})
})
