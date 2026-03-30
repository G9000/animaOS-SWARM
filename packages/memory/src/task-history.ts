import { BM25 } from "./bm25.js"

export interface TaskEntry {
	id: string
	agentId: string
	task: string
	result: string
	status: "success" | "error"
	timestamp: number
	durationMs: number
	tokensUsed: number
}

export class TaskHistory {
	private entries = new Map<string, TaskEntry>()
	private index = new BM25()

	record(entry: TaskEntry): void {
		this.entries.set(entry.id, entry)
		this.index.addDocument(entry.id, `${entry.task} ${entry.result}`)
	}

	search(query: string, limit = 10): Array<TaskEntry & { score: number }> {
		const results = this.index.search(query, limit)
		return results
			.map((r) => {
				const entry = this.entries.get(r.id)
				if (!entry) return null
				return { ...entry, score: r.score }
			})
			.filter((e): e is TaskEntry & { score: number } => e !== null)
	}

	getRecent(limit = 20): TaskEntry[] {
		return Array.from(this.entries.values())
			.sort((a, b) => b.timestamp - a.timestamp)
			.slice(0, limit)
	}

	getByAgent(agentId: string): TaskEntry[] {
		return Array.from(this.entries.values())
			.filter((e) => e.agentId === agentId)
			.sort((a, b) => b.timestamp - a.timestamp)
	}

	get(id: string): TaskEntry | undefined {
		return this.entries.get(id)
	}

	clear(): void {
		this.entries.clear()
		this.index.clear()
	}

	get size(): number {
		return this.entries.size
	}
}
