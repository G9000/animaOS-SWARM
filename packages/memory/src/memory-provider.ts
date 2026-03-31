import type { Provider, ProviderResult, IAgentRuntime, Message } from "@animaOS-SWARM/core"
import type { MemoryManager } from "./memory-manager.js"

/**
 * MemoryProvider — injects relevant memories into the agent's context before each LLM step.
 * Agents get situational awareness from past experience.
 */
export class MemoryProvider implements Provider {
	readonly name = "memory"
	readonly description = "Relevant memories from past interactions"

	constructor(private manager: MemoryManager) {}

	async get(runtime: IAgentRuntime, message: Message): Promise<ProviderResult> {
		const query = message.content.text ?? ""
		if (!query.trim()) return { text: "" }

		// Use agent name (stable across runs) not agentId (new UUID every run)
		const agentName = runtime.config.name

		// Search memories relevant to this agent and query
		const relevant = this.manager.search(query, {
			agentName,
			limit: 5,
			minImportance: 0.2,
		})

		// Also pull the 3 most recent observations regardless of relevance
		const recent = this.manager.getRecent({ agentName, limit: 3 })

		// Deduplicate: recent may overlap with relevant
		const seen = new Set(relevant.map((m) => m.id))
		const combined = [
			...relevant,
			...recent.filter((m) => !seen.has(m.id)),
		].slice(0, 7)

		if (combined.length === 0) return { text: "" }

		const lines = combined.map((m) => {
			const age = Math.floor((Date.now() - m.createdAt) / 60000) // minutes ago
			const ageStr = age < 60 ? `${age}m ago` : `${Math.floor(age / 60)}h ago`
			return `- [${m.type}] ${m.content} (${ageStr})`
		})

		return {
			text: ["## What You Remember", ...lines].join("\n"),
			metadata: { memoryCount: combined.length },
		}
	}
}
