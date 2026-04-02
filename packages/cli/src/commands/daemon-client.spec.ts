import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

const mockClient = {
	runTask: vi.fn(),
	createAgent: vi.fn(),
	runAgent: vi.fn(),
	listAgents: vi.fn(),
}

vi.mock("../client.js", () => ({
	createCliDaemonClient: vi.fn(() => mockClient),
}))

describe("CLI daemon-backed command cutover", () => {
	beforeEach(() => {
		vi.clearAllMocks()
		mockClient.runTask.mockReset()
		mockClient.createAgent.mockReset()
		mockClient.runAgent.mockReset()
		mockClient.listAgents.mockReset()
	})

	afterEach(() => {
		vi.restoreAllMocks()
	})

	it("run command delegates task execution to the daemon client", async () => {
		mockClient.runTask.mockResolvedValue({
			mode: "single",
			agent: {
				state: {
					id: "agent-1",
					name: "task-agent",
					status: "completed",
					config: {
						name: "task-agent",
						model: "gpt-4o-mini",
					},
					tokenUsage: {
						promptTokens: 20,
						completionTokens: 22,
						totalTokens: 42,
					},
					createdAt: Date.now(),
				},
				messageCount: 2,
				eventCount: 8,
				lastTask: null,
			},
			result: {
				status: "success",
				data: { text: "daemon handled task" },
				durationMs: 12,
			},
		})

		const logSpy = vi.spyOn(console, "log").mockImplementation(() => {})
		const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {})
		const { executeRunCommand } = await import("./run.js")

		await executeRunCommand("Ship the daemon cutover", {
			model: "gpt-4o-mini",
			provider: "openai",
			name: "task-agent",
			tui: false,
		})

		expect(mockClient.runTask).toHaveBeenCalledWith("Ship the daemon cutover", {
			model: "gpt-4o-mini",
			provider: "openai",
			name: "task-agent",
			strategy: undefined,
		})
		expect(logSpy).toHaveBeenCalled()
		expect(errorSpy).not.toHaveBeenCalled()
	})

	it("chat uses a single daemon-backed agent session", async () => {
		mockClient.createAgent.mockResolvedValue({
			state: {
				id: "agent-chat-1",
				name: "task-agent",
				status: "idle",
			},
		})
		mockClient.runAgent.mockResolvedValue({
			result: {
				status: "success",
				data: { text: "daemon reply" },
				durationMs: 8,
			},
		})

		const readline = {
			question: vi.fn(),
			close: vi.fn(),
		}
		const inputs = ["hello daemon", "exit"]
		readline.question.mockImplementation((_prompt: string, callback: (input: string) => void) => {
			callback(inputs.shift() ?? "exit")
		})

		const logSpy = vi.spyOn(console, "log").mockImplementation(() => {})
		const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {})
		const { executeChatCommand } = await import("./chat.js")

		await executeChatCommand(
			{
				model: "gpt-4o-mini",
				name: "task-agent",
			},
			{
				client: mockClient,
				createReadline: () => readline,
			},
		)

		expect(mockClient.createAgent).toHaveBeenCalledWith(
			expect.objectContaining({
				model: "gpt-4o-mini",
				name: "task-agent",
			}),
		)
		expect(mockClient.runAgent).toHaveBeenCalledWith("agent-chat-1", {
			text: "hello daemon",
		})
		expect(logSpy).toHaveBeenCalled()
		expect(errorSpy).not.toHaveBeenCalled()
	})

	it("agents list reads daemon-backed agent snapshots", async () => {
		mockClient.listAgents.mockResolvedValue([
			{
				state: {
					id: "agent-1",
					name: "planner",
					status: "idle",
					config: {
						name: "planner",
						model: "gpt-4o-mini",
					},
					tokenUsage: {
						promptTokens: 0,
						completionTokens: 0,
						totalTokens: 0,
					},
					createdAt: Date.now(),
				},
				messageCount: 0,
				eventCount: 1,
				lastTask: null,
			},
		])

		const logSpy = vi.spyOn(console, "log").mockImplementation(() => {})
		const { executeAgentsListCommand } = await import("./agents.js")

		await executeAgentsListCommand()

		expect(mockClient.listAgents).toHaveBeenCalled()
		expect(logSpy).toHaveBeenCalled()
	})
})
