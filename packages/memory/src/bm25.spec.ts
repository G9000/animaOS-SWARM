import { describe, it, expect } from "vitest"
import { BM25 } from "./bm25.js"

describe("BM25", () => {
	it("should index and search documents", () => {
		const bm25 = new BM25()
		bm25.addDocument("1", "The quick brown fox jumps over the lazy dog")
		bm25.addDocument("2", "A fast red car drives on the highway")
		bm25.addDocument("3", "The brown bear sleeps in the forest")

		const results = bm25.search("brown fox")
		expect(results.length).toBeGreaterThan(0)
		expect(results[0].id).toBe("1")
	})

	it("should rank relevant documents higher", () => {
		const bm25 = new BM25()
		bm25.addDocument("1", "TypeScript is a language")
		bm25.addDocument("2", "TypeScript TypeScript TypeScript everywhere, TypeScript is the best")
		bm25.addDocument("3", "Python is also a language")

		const results = bm25.search("TypeScript")
		expect(results.length).toBeGreaterThanOrEqual(2)
		// Doc 2 mentions TypeScript many times, should rank higher
		const doc1Score = results.find((r) => r.id === "1")?.score ?? 0
		const doc2Score = results.find((r) => r.id === "2")?.score ?? 0
		expect(doc2Score).toBeGreaterThanOrEqual(doc1Score)
	})

	it("should return empty for no matches", () => {
		const bm25 = new BM25()
		bm25.addDocument("1", "Hello world")

		const results = bm25.search("xyz123 nonexistent")
		expect(results).toHaveLength(0)
	})

	it("should handle document removal", () => {
		const bm25 = new BM25()
		bm25.addDocument("1", "agent swarm coordination")
		bm25.addDocument("2", "agent task execution")

		bm25.removeDocument("1")
		const results = bm25.search("agent")
		expect(results).toHaveLength(1)
		expect(results[0].id).toBe("2")
	})

	it("should handle empty queries", () => {
		const bm25 = new BM25()
		bm25.addDocument("1", "Hello world")

		const results = bm25.search("")
		expect(results).toHaveLength(0)
	})

	it("should clear all documents", () => {
		const bm25 = new BM25()
		bm25.addDocument("1", "test document")
		bm25.clear()

		expect(bm25.size).toBe(0)
		expect(bm25.search("test")).toHaveLength(0)
	})

	it("should update document on re-add", () => {
		const bm25 = new BM25()
		bm25.addDocument("1", "old content about cats")
		bm25.addDocument("1", "new content about dogs")

		expect(bm25.size).toBe(1)
		expect(bm25.search("cats")).toHaveLength(0)
		expect(bm25.search("dogs")).toHaveLength(1)
	})

	it("should respect limit", () => {
		const bm25 = new BM25()
		for (let i = 0; i < 20; i++) {
			bm25.addDocument(String(i), `document number ${i} about testing`)
		}

		const results = bm25.search("testing", 5)
		expect(results).toHaveLength(5)
	})
})
