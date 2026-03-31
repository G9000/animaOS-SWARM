import { randomUUID } from "node:crypto"
import { readFileSync, writeFileSync, existsSync } from "node:fs"
import { BM25 } from "./bm25.js"

export type MemoryType = "fact" | "observation" | "task_result" | "reflection"

export interface Memory {
	id: string
	agentId: string
	agentName: string
	type: MemoryType
	content: string
	importance: number // 0-1
	createdAt: number
	tags?: string[]
}

export interface MemorySearchResult extends Memory {
	score: number
}

export interface MemorySearchOptions {
	agentId?: string
	agentName?: string
	type?: MemoryType
	limit?: number
	minImportance?: number
}

export class MemoryManager {
	private memories = new Map<string, Memory>()
	private index = new BM25()
	private storageFile?: string

	constructor(storageFile?: string) {
		this.storageFile = storageFile
	}

	add(memory: Omit<Memory, "id" | "createdAt">): Memory {
		const full: Memory = {
			...memory,
			id: randomUUID(),
			createdAt: Date.now(),
		}
		this.memories.set(full.id, full)
		// Index: content + type + tags for BM25 search
		const indexText = [
			full.content,
			full.type,
			full.agentName,
			...(full.tags ?? []),
		].join(" ")
		this.index.addDocument(full.id, indexText)
		return full
	}

	search(query: string, opts: MemorySearchOptions = {}): MemorySearchResult[] {
		const { agentId, agentName, type, limit = 10, minImportance = 0 } = opts
		const raw = this.index.search(query, limit * 3) // over-fetch then filter

		const results: MemorySearchResult[] = []
		for (const r of raw) {
			const mem = this.memories.get(r.id)
			if (!mem) continue
			if (agentId && mem.agentId !== agentId) continue
			if (agentName && mem.agentName !== agentName) continue
			if (type && mem.type !== type) continue
			if (mem.importance < minImportance) continue
			results.push({ ...mem, score: r.score })
			if (results.length >= limit) break
		}
		return results
	}

	getRecent(opts: { agentId?: string; agentName?: string; limit?: number } = {}): Memory[] {
		const { agentId, agentName, limit = 20 } = opts
		return Array.from(this.memories.values())
			.filter((m) => {
				if (agentId && m.agentId !== agentId) return false
				if (agentName && m.agentName !== agentName) return false
				return true
			})
			.sort((a, b) => b.createdAt - a.createdAt)
			.slice(0, limit)
	}

	forget(id: string): void {
		this.memories.delete(id)
		this.index.removeDocument(id)
	}

	clear(agentId?: string): void {
		if (!agentId) {
			this.memories.clear()
			this.index.clear()
			return
		}
		for (const [id, mem] of this.memories) {
			if (mem.agentId === agentId) {
				this.memories.delete(id)
				this.index.removeDocument(id)
			}
		}
	}

	save(): void {
		if (!this.storageFile) return
		const data = Array.from(this.memories.values())
		writeFileSync(this.storageFile, JSON.stringify(data, null, 2))
	}

	load(): void {
		if (!this.storageFile || !existsSync(this.storageFile)) return
		try {
			const raw = readFileSync(this.storageFile, "utf-8")
			const data = JSON.parse(raw) as Memory[]
			for (const mem of data) {
				this.memories.set(mem.id, mem)
				const indexText = [mem.content, mem.type, mem.agentName, ...(mem.tags ?? [])].join(" ")
				this.index.addDocument(mem.id, indexText)
			}
		} catch {
			// Corrupted file — start fresh
		}
	}

	get size(): number {
		return this.memories.size
	}

	/** Return a summary line for display (e.g. TUI status) */
	summary(): string {
		return `${this.memories.size} memories`
	}
}
