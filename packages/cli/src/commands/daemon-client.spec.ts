import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { Command } from "commander"

const mockDaemonClient = {
	agents: {
		create: vi.fn(),
		run: vi.fn(),
		list: vi.fn(),
	},
	swarms: {
		create: vi.fn(),
		run: vi.fn(),
	},
}

vi.mock("../client.js", () => ({
	DaemonHttpError: class DaemonHttpError extends Error {
		constructor(
			public readonly status: number,
			public readonly body: unknown,
		) {
			super(
				typeof body === "object" &&
					body !== null &&
					"error" in body &&
					typeof body.error === "string"
						? body.error
						: `Daemon request failed with status ${status}`,
			)
		}
	},
	createCliDaemonClient: vi.fn(() => mockDaemonClient),
}))

vi.mock("./create.js", () => ({
	createCommand: new Command("create"),
}))

vi.mock("./launch.js", () => ({
	launchCommand: new Command("launch"),
}))

describe("CLI daemon-backed command cutover", () => {
	beforeEach(() => {
		vi.clearAllMocks()
		mockDaemonClient.agents.create.mockReset()
		mockDaemonClient.agents.run.mockReset()
		mockDaemonClient.agents.list.mockReset()
		mockDaemonClient.swarms.create.mockReset()
		mockDaemonClient.swarms.run.mockReset()
		process.exitCode = undefined
	})

	afterEach(() => {
		vi.restoreAllMocks()
		process.exitCode = undefined
	})

	it("run command delegates task execution to the daemon client", async () => {
		mockDaemonClient.agents.create.mockResolvedValue({
			state: {
				id: "agent-1",
				name: "task-agent",
				status: "idle",
				config: {
					name: "task-agent",
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
		})
		mockDaemonClient.agents.run.mockResolvedValue({
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

		expect(mockDaemonClient.agents.create).toHaveBeenCalledWith({
			model: "gpt-4o-mini",
			name: "task-agent",
			provider: "openai",
			system: "You are a helpful task agent. Use tools when needed. Be concise.",
		})
		expect(mockDaemonClient.agents.run).toHaveBeenCalledWith("agent-1", {
			text: "Ship the daemon cutover",
		})
		expect(logSpy).toHaveBeenCalled()
		expect(errorSpy).not.toHaveBeenCalled()
	})

	it("parses the real run CLI path without passing the commander instance as the daemon client", async () => {
		mockDaemonClient.agents.create.mockResolvedValue({
			state: {
				id: "agent-parse-1",
				name: "task-agent",
				status: "idle",
				config: {
					name: "task-agent",
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
		})
		mockDaemonClient.agents.run.mockResolvedValue({
			agent: {
				state: {
					id: "agent-parse-1",
					name: "task-agent",
					status: "completed",
					config: {
						name: "task-agent",
						model: "gpt-4o-mini",
					},
					tokenUsage: {
						promptTokens: 1,
						completionTokens: 1,
						totalTokens: 2,
					},
					createdAt: Date.now(),
				},
				messageCount: 2,
				eventCount: 8,
				lastTask: null,
			},
			result: {
				status: "success",
				data: { text: "parsed daemon path" },
				durationMs: 5,
			},
		})

		const logSpy = vi.spyOn(console, "log").mockImplementation(() => {})
		const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {})
		const { buildProgram } = await import("../index.js")

		await buildProgram().parseAsync(["node", "animaos", "run", "Parsed task"], {
			from: "node",
		})

		expect(mockDaemonClient.agents.create).toHaveBeenCalledOnce()
		expect(mockDaemonClient.agents.run).toHaveBeenCalledWith("agent-parse-1", {
			text: "Parsed task",
		})
		expect(logSpy).toHaveBeenCalled()
		expect(errorSpy).not.toHaveBeenCalled()
	})

	it("reports daemon failures without throwing from the run command", async () => {
		mockDaemonClient.agents.create.mockResolvedValue({
			state: {
				id: "agent-err-1",
				name: "task-agent",
				status: "idle",
				config: {
					name: "task-agent",
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
		})
		mockDaemonClient.agents.run.mockRejectedValue(new Error("daemon unavailable"))

		const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {})
		const { executeRunCommand } = await import("./run.js")

		await expect(
			executeRunCommand("Fail the daemon path", {
				model: "gpt-4o-mini",
				provider: "openai",
				name: "task-agent",
				tui: false,
			}),
		).resolves.toBeUndefined()

		expect(errorSpy).toHaveBeenCalledWith("Error:", "daemon unavailable")
		expect(process.exitCode).toBe(1)
	})

	it("fails fast when an unsupported api-key override is supplied to run", async () => {
		const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {})
		const { executeRunCommand } = await import("./run.js")

		await expect(
			executeRunCommand("Reject ignored api key", {
				model: "gpt-4o-mini",
				provider: "openai",
				name: "task-agent",
				apiKey: "secret",
				tui: false,
			}),
		).resolves.toBeUndefined()

		expect(mockDaemonClient.agents.create).not.toHaveBeenCalled()
		expect(mockDaemonClient.agents.run).not.toHaveBeenCalled()
		expect(errorSpy).toHaveBeenCalledWith(
			"Error:",
			"--api-key is not supported by the daemon-backed run command. Configure credentials in the daemon environment.",
		)
		expect(process.exitCode).toBe(1)
	})

	it("chat uses a single daemon-backed agent session without forcing a provider", async () => {
		mockDaemonClient.agents.create.mockResolvedValue({
			state: {
				id: "agent-chat-1",
				name: "task-agent",
				status: "idle",
			},
		})
		mockDaemonClient.agents.run.mockResolvedValue({
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
				client: mockDaemonClient,
				createReadline: () => readline,
			},
		)

		expect(mockDaemonClient.agents.create).toHaveBeenCalledWith({
			model: "gpt-4o-mini",
			name: "task-agent",
			system: "You are a helpful task agent. Use tools when needed. Be concise.",
		})
		expect(mockDaemonClient.agents.run).toHaveBeenCalledWith("agent-chat-1", {
			text: "hello daemon",
		})
		expect(logSpy).toHaveBeenCalled()
		expect(errorSpy).not.toHaveBeenCalled()
	})

	it("fails fast when an unsupported api-key override is supplied to chat", async () => {
		const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {})
		const { executeChatCommand } = await import("./chat.js")

		await expect(
			executeChatCommand(
				{
					model: "gpt-4o-mini",
					name: "task-agent",
					apiKey: "secret",
				},
				{
					client: mockDaemonClient,
				},
			),
		).resolves.toBeUndefined()

		expect(mockDaemonClient.agents.create).not.toHaveBeenCalled()
		expect(errorSpy).toHaveBeenCalledWith(
			"Error:",
			"--api-key is not supported by the daemon-backed chat command. Configure credentials in the daemon environment.",
		)
		expect(process.exitCode).toBe(1)
	})

	it("agents list reads daemon-backed agent snapshots", async () => {
		mockDaemonClient.agents.list.mockResolvedValue([
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

		expect(mockDaemonClient.agents.list).toHaveBeenCalled()
		expect(logSpy).toHaveBeenCalled()
	})
})
