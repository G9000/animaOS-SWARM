import { describe, it, expect, vi, beforeEach, afterEach } from "vitest"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { existsSync, unlinkSync } from "node:fs"
import { MemoryManager } from "./memory-manager.js"
import { ObservationEvaluator } from "./observation-evaluator.js"
import type { IAgentRuntime, Message, AgentConfig, Content } from "@animaOS-SWARM/core"

// ─── helpers ────────────────────────────────────────────────────────────────

function makeRuntime(name = "researcher", agentId = "agent-uuid-1"): IAgentRuntime {
	return {
		agentId: agentId as IAgentRuntime["agentId"],
		config: { name, model: "test-model" } as AgentConfig,
		run: vi.fn(),
		getActions: vi.fn().mockReturnValue([]),
		registerPlugin: vi.fn(),
		send: vi.fn(),
		spawn: vi.fn(),
		broadcast: vi.fn(),
		stop: vi.fn(),
	}
}

function makeMessage(text: string): Message {
	return {
		id: "msg-uuid-1-abc-def-g" as Message["id"],
		agentId: "agent-uuid-1" as Message["agentId"],
		roomId: "room-uuid-1" as Message["roomId"],
		content: { text },
		role: "user",
		createdAt: Date.now(),
	}
}

function makeContent(text: string): Content {
	return { text }
}

function tmpFile(): string {
	return join(tmpdir(), `anima-test-obs-${Date.now()}-${Math.random().toString(36).slice(2)}.json`)
}

// ─── validate() ─────────────────────────────────────────────────────────────

describe("ObservationEvaluator.validate()", () => {
	it("always returns true regardless of input", async () => {
		const manager = new MemoryManager()
		const ev = new ObservationEvaluator(manager)
		const runtime = makeRuntime()

		expect(await ev.validate(runtime, makeMessage("any task"))).toBe(true)
		expect(await ev.validate(runtime, makeMessage(""))).toBe(true)
	})
})

// ─── handler() ──────────────────────────────────────────────────────────────

describe("ObservationEvaluator.handler()", () => {
	let manager: MemoryManager
	let ev: ObservationEvaluator

	beforeEach(() => {
		manager = new MemoryManager()
		ev = new ObservationEvaluator(manager)
	})

	it("stores a task_result memory after a successful run", async () => {
		const runtime = makeRuntime()
		await ev.handler(runtime, makeMessage("Analyse the codebase"), makeContent("The codebase uses TypeScript with Nx."))

		expect(manager.size).toBeGreaterThan(0)
		const results = manager.search("codebase")
		const taskResult = results.find((r) => r.type === "task_result")
		expect(taskResult).toBeDefined()
	})

	it("task_result content includes both the task and the result", async () => {
		const runtime = makeRuntime()
		await ev.handler(
			runtime,
			makeMessage("List all packages"),
			makeContent("There are five packages: core, cli, tui, memory, swarm."),
		)

		const results = manager.search("packages", { type: "task_result" })
		expect(results.length).toBeGreaterThan(0)
		expect(results[0].content).toContain("List all packages")
		expect(results[0].content).toContain("There are five packages")
	})

	it("task_result is stored with importance 0.6", async () => {
		const runtime = makeRuntime()
		await ev.handler(runtime, makeMessage("Do something"), makeContent("Done."))

		const results = manager.search("something", { type: "task_result" })
		expect(results[0].importance).toBe(0.6)
	})

	it("tags the task_result with the agent's name", async () => {
		const runtime = makeRuntime("writer")
		await ev.handler(runtime, makeMessage("Write a summary"), makeContent("Summary written."))

		const all = manager.getRecent({ agentName: "writer" })
		const writerTask = all.find((m) => m.agentName === "writer" && m.type === "task_result")
		expect(writerTask).toBeDefined()
		expect(writerTask?.tags).toContain("writer")
	})

	it("stores an observation when result is longer than 100 chars", async () => {
		const runtime = makeRuntime()
		const longResult = "The Nx workspace contains multiple packages organized by domain. Each package has its own build targets."
		expect(longResult.length).toBeGreaterThan(100)

		await ev.handler(runtime, makeMessage("Describe the workspace"), makeContent(longResult))

		const observations = manager.getRecent().filter((m) => m.type === "observation")
		expect(observations.length).toBeGreaterThan(0)
	})

	it("does NOT store an observation when result is 100 chars or fewer", async () => {
		const runtime = makeRuntime()
		const shortResult = "Done. Task completed successfully."
		expect(shortResult.length).toBeLessThanOrEqual(100)

		await ev.handler(runtime, makeMessage("Short task"), makeContent(shortResult))

		const observations = manager.getRecent().filter((m) => m.type === "observation")
		expect(observations).toHaveLength(0)
	})

	it("observation content is the first sentence of the result", async () => {
		const runtime = makeRuntime()
		// Must be > 100 chars total so the evaluator extracts a first-sentence observation.
		// The source splits on /[.!?]\s/ — first sentence ends at ". " after "scoring".
		const result = "The system uses BM25 for search ranking and relevance scoring. It also has persistence via JSON files. And even more features beyond that."
		expect(result.length).toBeGreaterThan(100)
		await ev.handler(runtime, makeMessage("Describe the system"), makeContent(result))

		const observations = manager.getRecent().filter((m) => m.type === "observation")
		expect(observations.length).toBeGreaterThan(0)
		// First sentence only: everything before the first ". "
		expect(observations[0].content).toContain("The system uses BM25 for search ranking and relevance scoring")
		// Second and third sentences must NOT be included in the observation
		expect(observations[0].content).not.toContain("It also has persistence")
		expect(observations[0].content).not.toContain("And even more features beyond that")
	})

	it("skips observation if first sentence is too short (<=20 chars)", async () => {
		const runtime = makeRuntime()
		// Result is >100 chars but first sentence is very short
		const result = "OK. " + "x".repeat(100)
		await ev.handler(runtime, makeMessage("Quick task"), makeContent(result))

		const observations = manager.getRecent().filter((m) => m.type === "observation")
		expect(observations).toHaveLength(0)
	})

	it("is a no-op when the task is blank — returns empty object, stores nothing", async () => {
		const runtime = makeRuntime()
		const result = await ev.handler(runtime, makeMessage(""), makeContent("Some result"))

		expect(manager.size).toBe(0)
		expect(result).toEqual({}) // early-return path: no metadata, no score
	})

	it("is a no-op when the result is blank — returns empty object, stores nothing", async () => {
		const runtime = makeRuntime()
		const result = await ev.handler(runtime, makeMessage("Do something"), makeContent(""))

		expect(manager.size).toBe(0)
		expect(result).toEqual({})
	})

	it("is a no-op when both task and result are whitespace only — returns empty object", async () => {
		const runtime = makeRuntime()
		const result = await ev.handler(runtime, makeMessage("  "), makeContent("  "))

		expect(manager.size).toBe(0)
		expect(result).toEqual({})
	})

	it("returns metadata with memorized: true", async () => {
		const runtime = makeRuntime()
		const result = await ev.handler(
			runtime,
			makeMessage("Analyse something"),
			makeContent("The analysis is complete and thorough."),
		)

		expect(result.metadata?.memorized).toBe(true)
	})

	it("stores the correct agentId and agentName on the memory", async () => {
		const runtime = makeRuntime("reviewer", "reviewer-uuid-999")
		await ev.handler(
			runtime,
			makeMessage("Review the code"),
			makeContent("Code review is done. Found two issues."),
		)

		const all = manager.getRecent()
		for (const m of all) {
			expect(m.agentId).toBe("reviewer-uuid-999")
			expect(m.agentName).toBe("reviewer")
		}
	})

	it("truncates very long tasks to 120 chars in the task_result content", async () => {
		const runtime = makeRuntime()
		const longTask = "a".repeat(200) // 200 chars
		await ev.handler(runtime, makeMessage(longTask), makeContent("Done with the long task."))

		const results = manager.search("task", { type: "task_result" })
		// The stored content should not contain 200 'a' chars — it's capped at 120
		const taskPart = results[0]?.content ?? ""
		expect(taskPart).not.toContain("a".repeat(121))
	})

	it("truncates very long results to 300 chars in the task_result content", async () => {
		const runtime = makeRuntime()
		const longResult = "b".repeat(400) // 400 chars
		await ev.handler(runtime, makeMessage("Short task"), makeContent(longResult))

		const results = manager.search("Short task", { type: "task_result" })
		const content = results[0]?.content ?? ""
		expect(content).not.toContain("b".repeat(301))
	})

	it("multiple handler calls accumulate memories independently", async () => {
		const runtime = makeRuntime()
		await ev.handler(runtime, makeMessage("Task one"), makeContent("Result one."))
		await ev.handler(runtime, makeMessage("Task two"), makeContent("Result two."))
		await ev.handler(runtime, makeMessage("Task three"), makeContent("Result three."))

		const taskResults = manager.getRecent().filter((m) => m.type === "task_result")
		expect(taskResults).toHaveLength(3)
	})
})

// ─── handler() calls save() ──────────────────────────────────────────────────

describe("ObservationEvaluator.handler() persistence", () => {
	let file: string

	beforeEach(() => { file = tmpFile() })
	afterEach(() => { if (existsSync(file)) unlinkSync(file) })

	it("calls manager.save() after each observation", async () => {
		const manager = new MemoryManager(file)
		const saveSpy = vi.spyOn(manager, "save")
		const ev = new ObservationEvaluator(manager)
		const runtime = makeRuntime()

		await ev.handler(runtime, makeMessage("Save this"), makeContent("Saved!"))

		expect(saveSpy).toHaveBeenCalledOnce()
	})

	it("persists memories to disk so a reloaded manager can access them", async () => {
		const manager = new MemoryManager(file)
		const ev = new ObservationEvaluator(manager)
		const runtime = makeRuntime("researcher")

		await ev.handler(
			runtime,
			makeMessage("What is Nx?"),
			makeContent("Nx is a powerful build system for monorepos with smart caching."),
		)

		// Reload from disk
		const reloaded = new MemoryManager(file)
		reloaded.load()

		expect(reloaded.size).toBeGreaterThan(0)
		const results = reloaded.search("Nx build system")
		expect(results.length).toBeGreaterThan(0)
	})

	it("save() is called even when no observation is extracted (short result)", async () => {
		const manager = new MemoryManager(file)
		const saveSpy = vi.spyOn(manager, "save")
		const ev = new ObservationEvaluator(manager)
		const runtime = makeRuntime()

		// Short result: only task_result stored, no observation
		await ev.handler(runtime, makeMessage("Quick task"), makeContent("Done."))

		expect(saveSpy).toHaveBeenCalledOnce()
		expect(existsSync(file)).toBe(true)
	})
})
