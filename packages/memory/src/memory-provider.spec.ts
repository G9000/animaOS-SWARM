import { describe, it, expect, vi, beforeEach } from "vitest"
import { MemoryManager } from "./memory-manager.js"
import { MemoryProvider } from "./memory-provider.js"
import type { IAgentRuntime, Message, AgentConfig } from "@animaOS-SWARM/core"

// ─── helpers ────────────────────────────────────────────────────────────────

function makeRuntime(name: string, agentId = "agent-uuid-1"): IAgentRuntime {
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
		id: "msg-1-uuid-abc-def-ghi" as Message["id"],
		agentId: "agent-uuid-1" as Message["agentId"],
		roomId: "room-uuid-1" as Message["roomId"],
		content: { text },
		role: "user",
		createdAt: Date.now(),
	}
}

// ─── MemoryProvider.get() ────────────────────────────────────────────────────

describe("MemoryProvider.get()", () => {
	let manager: MemoryManager
	let provider: MemoryProvider

	beforeEach(() => {
		manager = new MemoryManager()
		provider = new MemoryProvider(manager)
	})

	it("returns empty text when the memory store is empty", async () => {
		const runtime = makeRuntime("researcher")
		const result = await provider.get(runtime, makeMessage("what do we know about TypeScript"))
		expect(result.text).toBe("")
	})

	it("returns empty text for a blank query", async () => {
		manager.add({ agentId: "a1", agentName: "researcher", type: "fact", content: "TypeScript is great", importance: 0.8 })
		const runtime = makeRuntime("researcher")
		const result = await provider.get(runtime, makeMessage("   "))
		expect(result.text).toBe("")
	})

	it("returns non-empty text when relevant memories exist", async () => {
		const runtime = makeRuntime("researcher")
		manager.add({ agentId: "a1", agentName: "researcher", type: "fact", content: "TypeScript is a statically typed language", importance: 0.8 })

		const result = await provider.get(runtime, makeMessage("TypeScript"))
		expect(result.text.length).toBeGreaterThan(0)
	})

	it("formats output with ## What You Remember header", async () => {
		const runtime = makeRuntime("researcher")
		manager.add({ agentId: "a1", agentName: "researcher", type: "fact", content: "React is a UI library", importance: 0.8 })

		const result = await provider.get(runtime, makeMessage("React library"))
		expect(result.text).toContain("## What You Remember")
	})

	it("formats each memory as '- [type] content (age)'", async () => {
		const runtime = makeRuntime("researcher")
		manager.add({ agentId: "a1", agentName: "researcher", type: "fact", content: "BM25 is a ranking algorithm", importance: 0.8 })

		const result = await provider.get(runtime, makeMessage("BM25 ranking"))
		// Should have a line matching: - [fact] BM25 is a ranking algorithm (Xm ago)
		expect(result.text).toMatch(/- \[fact\] BM25 is a ranking algorithm \(\d+m ago\)/)
	})

	it("scopes search to the agent's own name", async () => {
		// Add memories for two different agents
		manager.add({ agentId: "a1", agentName: "researcher", type: "fact", content: "researcher knows TypeScript deeply", importance: 0.9 })
		manager.add({ agentId: "a2", agentName: "writer", type: "fact", content: "writer knows TypeScript basics", importance: 0.9 })

		const runtime = makeRuntime("researcher")
		const result = await provider.get(runtime, makeMessage("TypeScript"))

		// Should only include researcher's memories
		expect(result.text).toContain("researcher knows TypeScript deeply")
		expect(result.text).not.toContain("writer knows TypeScript basics")
	})

	it("uses agentName (not agentId) for scoping so memories persist across runs", async () => {
		// Agent "researcher" stores a memory with agentId "old-uuid"
		manager.add({ agentId: "old-uuid", agentName: "researcher", type: "fact", content: "Nx is a monorepo build system", importance: 0.8 })

		// Same agent, new spawn → new UUID but same name
		const runtime = makeRuntime("researcher", "new-uuid")
		const result = await provider.get(runtime, makeMessage("Nx monorepo"))

		// Memory stored by "old-uuid" should still be accessible because agentName matches
		expect(result.text).toContain("Nx is a monorepo build system")
	})

	it("includes recent memories via getRecent() even when unrelated to the query", async () => {
		const runtime = makeRuntime("researcher")
		manager.add({ agentId: "a1", agentName: "researcher", type: "task_result", content: "completed task alpha", importance: 0.5 })
		manager.add({ agentId: "a1", agentName: "researcher", type: "task_result", content: "completed task beta", importance: 0.5 })
		manager.add({ agentId: "a1", agentName: "researcher", type: "task_result", content: "completed task gamma", importance: 0.5 })

		// Query completely unrelated to those tasks — BM25 returns zero search hits.
		// But getRecent() (limit: 3) should still pull them in.
		const result = await provider.get(runtime, makeMessage("quantum physics neutron stars"))
		expect(result.text).toContain("completed task alpha")
		expect(result.text).toContain("completed task beta")
		expect(result.text).toContain("completed task gamma")
	})

	it("deduplicates memories that appear in both search results and recent", async () => {
		const runtime = makeRuntime("researcher")
		manager.add({ agentId: "a1", agentName: "researcher", type: "fact", content: "TypeScript has generics", importance: 0.9 })
		manager.add({ agentId: "a1", agentName: "researcher", type: "fact", content: "TypeScript has interfaces", importance: 0.9 })

		const result = await provider.get(runtime, makeMessage("TypeScript generics interfaces"))
		// Count occurrences — a deduplicated result won't repeat the same content
		const occurrences = (result.text.match(/TypeScript has generics/g) ?? []).length
		expect(occurrences).toBe(1)
	})

	it("caps combined output at 7 memories maximum", async () => {
		const runtime = makeRuntime("researcher")
		// Add 10 memories all relevant to the query
		for (let i = 1; i <= 10; i++) {
			manager.add({ agentId: "a1", agentName: "researcher", type: "fact", content: `TypeScript fact number ${i}`, importance: 0.8 })
		}

		const result = await provider.get(runtime, makeMessage("TypeScript"))
		// Count lines starting with "- [" — each is one memory
		const lines = result.text.split("\n").filter((l) => l.startsWith("- ["))
		expect(lines.length).toBeLessThanOrEqual(7)
	})

	it("returns metadata.memoryCount equal to the exact number of memories included", async () => {
		const runtime = makeRuntime("researcher")
		manager.add({ agentId: "a1", agentName: "researcher", type: "fact", content: "Vitest is a fast test runner", importance: 0.8 })
		manager.add({ agentId: "a1", agentName: "researcher", type: "observation", content: "Vitest integrates with Vite", importance: 0.7 })

		const result = await provider.get(runtime, makeMessage("Vitest testing"))
		// Both memories are scoped to "researcher" and relevant to the query — expect exactly 2
		expect(result.metadata?.memoryCount).toBe(2)
	})

	it("search() excludes memories below minImportance 0.2; getRecent() may still include them", async () => {
		// MemoryProvider applies minImportance: 0.2 only to the search() call.
		// getRecent() (limit: 3) does NOT filter by importance.
		// Strategy: fill all 3 recent slots + 5 search slots with high-importance memories,
		// leaving no room for the low-importance one in the 7-item combined cap.
		const runtime = makeRuntime("researcher")

		// 5 high-importance memories relevant to the query — fill search slots
		for (let i = 1; i <= 5; i++) {
			manager.add({ agentId: "a1", agentName: "researcher", type: "fact", content: `high importance fact ${i}`, importance: 0.9 })
		}
		// Low-importance memory that BM25 will not rank highly for this query
		manager.add({ agentId: "a1", agentName: "researcher", type: "fact", content: "barely relevant obscure widget detail", importance: 0.1 })
		// 3 more high-importance memories added after — fill recent slots
		for (let i = 6; i <= 8; i++) {
			manager.add({ agentId: "a1", agentName: "researcher", type: "fact", content: `high importance fact ${i}`, importance: 0.9 })
		}

		const result = await provider.get(runtime, makeMessage("high importance fact"))
		expect(result.text).toContain("high importance fact")
		// Low-importance memory is excluded by minImportance: 0.2 from search,
		// and the 7-item cap is already filled by high-importance results
		expect(result.text).not.toContain("barely relevant obscure widget detail")
	})
})
