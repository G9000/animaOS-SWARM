import { describe, it, expect, beforeEach, afterEach, vi } from "vitest"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { unlinkSync, existsSync, writeFileSync } from "node:fs"
import { MemoryManager } from "./memory-manager.js"
import type { NewMemoryInput } from "./memory-manager.js"

// ─── helpers ────────────────────────────────────────────────────────────────

function base(overrides: Partial<NewMemoryInput> = {}): NewMemoryInput {
	return {
		agentId: "agent-1",
		agentName: "researcher",
		type: "fact",
		content: "TypeScript is a statically typed language",
		importance: 0.5,
		...overrides,
	}
}

function tmpFile(): string {
	return join(tmpdir(), `anima-test-memory-${Date.now()}-${Math.random().toString(36).slice(2)}.json`)
}

// ─── add() ──────────────────────────────────────────────────────────────────

describe("MemoryManager.add()", () => {
	let manager: MemoryManager

	beforeEach(() => { manager = new MemoryManager() })

	it("assigns a unique id to each memory", () => {
		const a = manager.add(base({ content: "fact one" }))
		const b = manager.add(base({ content: "fact two" }))
		expect(a.id).toBeDefined()
		expect(b.id).toBeDefined()
		expect(a.id).not.toBe(b.id)
	})

	it("sets createdAt to roughly now", () => {
		const before = Date.now()
		const m = manager.add(base())
		const after = Date.now()
		expect(m.createdAt).toBeGreaterThanOrEqual(before)
		expect(m.createdAt).toBeLessThanOrEqual(after)
	})

	it("preserves all provided fields unchanged", () => {
		const m = manager.add({
			agentId: "a99",
			agentName: "writer",
			type: "task_result",
			content: "Task was completed successfully",
			importance: 0.9,
			tags: ["done", "verified"],
		})
		expect(m.agentId).toBe("a99")
		expect(m.agentName).toBe("writer")
		expect(m.type).toBe("task_result")
		expect(m.content).toBe("Task was completed successfully")
		expect(m.importance).toBe(0.9)
		expect(m.tags).toEqual(["done", "verified"])
	})

	it("increments size with each add", () => {
		expect(manager.size).toBe(0)
		manager.add(base())
		expect(manager.size).toBe(1)
		manager.add(base())
		expect(manager.size).toBe(2)
	})

	it("makes memories immediately searchable", () => {
		manager.add(base({ content: "pglite is an in-process SQLite database" }))
		const results = manager.search("SQLite database")
		expect(results.length).toBeGreaterThan(0)
		expect(results[0].content).toContain("pglite")
	})
})

// ─── search() ───────────────────────────────────────────────────────────────

describe("MemoryManager.search()", () => {
	let manager: MemoryManager

	beforeEach(() => {
		manager = new MemoryManager()
		manager.add(base({ agentId: "a1", agentName: "researcher", type: "fact",        content: "TypeScript is a statically typed superset of JavaScript",    importance: 0.9 }))
		manager.add(base({ agentId: "a1", agentName: "researcher", type: "observation", content: "React hooks simplify stateful component logic",               importance: 0.7 }))
		manager.add(base({ agentId: "a2", agentName: "writer",     type: "fact",        content: "BM25 is a probabilistic ranking algorithm for text search",   importance: 0.8 }))
		manager.add(base({ agentId: "a2", agentName: "writer",     type: "task_result", content: "Wrote API documentation covering 12 endpoints",              importance: 0.3 }))
		manager.add(base({ agentId: "a3", agentName: "reviewer",   type: "reflection",  content: "Code review revealed three potential null pointer exceptions", importance: 0.6 }))
	})

	it("returns relevant results for a query", () => {
		const results = manager.search("TypeScript JavaScript typed")
		expect(results.length).toBeGreaterThan(0)
		expect(results[0].content).toContain("TypeScript")
	})

	it("returns results with a positive score attached", () => {
		const results = manager.search("TypeScript")
		for (const r of results) {
			expect(r.score).toBeGreaterThan(0)
		}
	})

	it("ranks more relevant results higher", () => {
		const results = manager.search("BM25 ranking algorithm text search")
		expect(results[0].content).toContain("BM25")
	})

	it("returns empty array when nothing matches", () => {
		const results = manager.search("quantum entanglement neutron stars")
		expect(results).toHaveLength(0)
	})

	it("returns empty array for a blank query", () => {
		const results = manager.search("")
		expect(results).toHaveLength(0)
	})

	it("filters by agentId", () => {
		const results = manager.search("code review documentation", { agentId: "a2" })
		expect(results.length).toBeGreaterThan(0)
		for (const r of results) expect(r.agentId).toBe("a2")
	})

	it("returns nothing when agentId has no matches", () => {
		const results = manager.search("TypeScript", { agentId: "nonexistent" })
		expect(results).toHaveLength(0)
	})

	it("filters by agentName", () => {
		const results = manager.search("TypeScript React hooks", { agentName: "researcher" })
		expect(results.length).toBeGreaterThan(0)
		for (const r of results) expect(r.agentName).toBe("researcher")
	})

	it("filters by type", () => {
		const results = manager.search("code endpoints documentation", { type: "task_result" })
		expect(results.length).toBeGreaterThan(0) // guard: loop is vacuous if empty
		for (const r of results) expect(r.type).toBe("task_result")
	})

	it("filters out memories below minImportance", () => {
		// Use a query that matches BOTH high-importance (TypeScript: 0.9, code review: 0.6)
		// AND the low-importance task_result (documentation: 0.3), so the filter is actually exercised.
		// Without the guard this test was vacuously passing — "documentation API endpoints" only
		// matched the 0.3 doc which was then excluded, leaving 0 results and no assertions running.
		const results = manager.search("code review documentation TypeScript", { minImportance: 0.5 })
		expect(results.length).toBeGreaterThan(0) // guard: ensures the loop below actually runs
		for (const r of results) expect(r.importance).toBeGreaterThanOrEqual(0.5)
	})

	it("returns all importances when minImportance is 0", () => {
		const results = manager.search("documentation", { minImportance: 0 })
		const hasLow = results.some((r) => r.importance < 0.5)
		expect(hasLow).toBe(true)
	})

	it("respects the limit option", () => {
		const results = manager.search("code", { limit: 2 })
		expect(results.length).toBeLessThanOrEqual(2)
	})

	it("can combine multiple filters simultaneously", () => {
		const results = manager.search("BM25 algorithm", {
			agentName: "writer",
			type: "fact",
			minImportance: 0.5,
			limit: 5,
		})
		for (const r of results) {
			expect(r.agentName).toBe("writer")
			expect(r.type).toBe("fact")
			expect(r.importance).toBeGreaterThanOrEqual(0.5)
		}
	})
})

// ─── getRecent() ─────────────────────────────────────────────────────────────

describe("MemoryManager.getRecent()", () => {
	let manager: MemoryManager

	beforeEach(() => { manager = new MemoryManager() })

	it("returns memories sorted newest first", () => {
		const now = 1_700_000_000_000
		vi.useFakeTimers()
		vi.setSystemTime(now)
		manager.add(base({ content: "oldest" }))
		vi.setSystemTime(now + 1000)
		manager.add(base({ content: "middle" }))
		vi.setSystemTime(now + 2000)
		manager.add(base({ content: "newest" }))
		vi.useRealTimers()

		const recent = manager.getRecent()
		expect(recent[0].content).toBe("newest")
		expect(recent[1].content).toBe("middle")
		expect(recent[2].content).toBe("oldest")
	})

	it("respects the limit", () => {
		manager.add(base({ content: "a" }))
		manager.add(base({ content: "b" }))
		manager.add(base({ content: "c" }))
		manager.add(base({ content: "d" }))

		const recent = manager.getRecent({ limit: 2 })
		expect(recent).toHaveLength(2)
	})

	it("filters by agentId", () => {
		manager.add(base({ agentId: "a1", agentName: "agent-a", content: "a1 memory" }))
		manager.add(base({ agentId: "a2", agentName: "agent-b", content: "a2 memory" }))
		manager.add(base({ agentId: "a1", agentName: "agent-a", content: "a1 again" }))

		const recent = manager.getRecent({ agentId: "a1" })
		expect(recent).toHaveLength(2)
		for (const r of recent) expect(r.agentId).toBe("a1")
	})

	it("filters by agentName", () => {
		manager.add(base({ agentName: "researcher", content: "research memory" }))
		manager.add(base({ agentName: "writer",     content: "writing memory" }))
		manager.add(base({ agentName: "researcher", content: "more research" }))

		const recent = manager.getRecent({ agentName: "researcher" })
		expect(recent).toHaveLength(2)
		for (const r of recent) expect(r.agentName).toBe("researcher")
	})

	it("returns empty array when no memories exist", () => {
		expect(manager.getRecent()).toHaveLength(0)
	})
})

// ─── forget() ────────────────────────────────────────────────────────────────

describe("MemoryManager.forget()", () => {
	let manager: MemoryManager

	beforeEach(() => { manager = new MemoryManager() })

	it("removes the memory from the store", () => {
		const m = manager.add(base({ content: "temporary fact" }))
		expect(manager.size).toBe(1)
		manager.forget(m.id)
		expect(manager.size).toBe(0)
	})

	it("removes the memory from the search index", () => {
		const m = manager.add(base({ content: "pglite is an in-process database" }))
		manager.forget(m.id)
		const results = manager.search("pglite in-process database")
		expect(results).toHaveLength(0)
	})

	it("leaves other memories intact", () => {
		const a = manager.add(base({ content: "memory A about TypeScript" }))
		manager.add(base({ content: "memory B about React" }))
		manager.forget(a.id)

		expect(manager.size).toBe(1)
		const results = manager.search("React")
		expect(results).toHaveLength(1)
		expect(results[0].content).toContain("React")
	})

	it("is a no-op for a non-existent id", () => {
		manager.add(base())
		expect(() => manager.forget("non-existent-id")).not.toThrow()
		expect(manager.size).toBe(1)
	})
})

// ─── clear() ─────────────────────────────────────────────────────────────────

describe("MemoryManager.clear()", () => {
	let manager: MemoryManager

	beforeEach(() => {
		manager = new MemoryManager()
		manager.add(base({ agentId: "a1", agentName: "agent-a", content: "agent A fact 1" }))
		manager.add(base({ agentId: "a1", agentName: "agent-a", content: "agent A fact 2" }))
		manager.add(base({ agentId: "a2", agentName: "agent-b", content: "agent B fact" }))
	})

	it("clears all memories when called without arguments", () => {
		manager.clear()
		expect(manager.size).toBe(0)
		expect(manager.search("fact")).toHaveLength(0)
	})

	it("clears only a specific agent's memories by agentId", () => {
		manager.clear("a1")
		expect(manager.size).toBe(1)
		expect(manager.getRecent()[0].agentId).toBe("a2")
	})

	it("removes cleared agent's memories from search index", () => {
		manager.clear("a1")
		// "agent B fact" is the only remaining memory — searching "agent" should find it
		const results = manager.search("agent B fact")
		expect(results.length).toBeGreaterThan(0) // guard: loop is vacuous if empty
		for (const r of results) expect(r.agentId).toBe("a2")
	})
})

// ─── save() / load() ─────────────────────────────────────────────────────────

describe("MemoryManager persistence", () => {
	let file: string
	let manager: MemoryManager

	beforeEach(() => {
		file = tmpFile()
		manager = new MemoryManager(file)
	})

	afterEach(() => {
		if (existsSync(file)) unlinkSync(file)
	})

	it("save() writes memories to a JSON file", () => {
		manager.add(base({ content: "saved fact" }))
		manager.save()
		expect(existsSync(file)).toBe(true)
	})

	it("load() restores memories from a JSON file", () => {
		manager.add(base({ content: "persisted memory" }))
		manager.add(base({ content: "another persisted memory", agentName: "writer" }))
		manager.save()

		const reloaded = new MemoryManager(file)
		reloaded.load()

		expect(reloaded.size).toBe(2)
	})

	it("load() restores the search index so search works after reload", () => {
		manager.add(base({ content: "Nx is a build system for monorepos" }))
		manager.save()

		const reloaded = new MemoryManager(file)
		reloaded.load()

		const results = reloaded.search("Nx monorepo build")
		expect(results.length).toBeGreaterThan(0)
		expect(results[0].content).toContain("Nx")
	})

	it("load() preserves all memory fields including id and createdAt", () => {
		const original = manager.add(base({ content: "to be preserved" }))
		manager.save()

		const reloaded = new MemoryManager(file)
		reloaded.load()

		const restored = reloaded.getRecent()[0]
		expect(restored.id).toBe(original.id)
		expect(restored.createdAt).toBe(original.createdAt)
		expect(restored.content).toBe(original.content)
	})

	it("load() is a no-op when the file does not exist", () => {
		const missing = new MemoryManager("/nonexistent/path/memory.json")
		expect(() => missing.load()).not.toThrow()
		expect(missing.size).toBe(0)
	})

	it("load() is a no-op when called without a storage file configured", () => {
		const noFile = new MemoryManager()
		expect(() => noFile.load()).not.toThrow()
	})

	it("save() is a no-op when called without a storage file configured", () => {
		const noFile = new MemoryManager()
		noFile.add(base())
		expect(() => noFile.save()).not.toThrow()
	})

	it("load() recovers gracefully from a corrupted file", () => {
		writeFileSync(file, "{ this is not valid JSON }")
		const bad = new MemoryManager(file)
		expect(() => bad.load()).not.toThrow()
		expect(bad.size).toBe(0)
	})

	it("calling load() twice is idempotent — no duplicate entries", () => {
		manager.add(base({ content: "unique memory" }))
		manager.save()

		const reloaded = new MemoryManager(file)
		reloaded.load()
		reloaded.load()

		expect(reloaded.size).toBe(1)
	})

	it("save() can be called multiple times safely", () => {
		manager.add(base({ content: "fact one" }))
		manager.save()
		manager.add(base({ content: "fact two" }))
		manager.save()

		const reloaded = new MemoryManager(file)
		reloaded.load()
		expect(reloaded.size).toBe(2)
	})
})

// ─── summary() / size ────────────────────────────────────────────────────────

describe("MemoryManager.summary() and .size", () => {
	it("summary() reflects the current count — BUG: always uses plural 'memories' even for 1", () => {
		// The source returns `"${size} memories"` unconditionally.
		// "1 memories" is grammatically incorrect; the correct form would be "1 memory".
		// This test pins the current behavior. Update both the source and this test when fixed.
		const manager = new MemoryManager()
		expect(manager.summary()).toBe("0 memories")
		expect(manager.summary()).not.toBe("1 memory") // documents what correct behavior would look like
		manager.add(base())
		expect(manager.summary()).toBe("1 memories") // BUG: should be "1 memory"
		manager.add(base())
		expect(manager.summary()).toBe("2 memories")
	})

	it("size returns 0 for a fresh instance", () => {
		expect(new MemoryManager().size).toBe(0)
	})
})
