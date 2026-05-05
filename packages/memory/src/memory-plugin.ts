import type { Plugin, IAgentRuntime, Message } from "@animaOS-SWARM/core"
import { MemoryManager } from "./memory-manager.js"
import { MemoryProvider } from "./memory-provider.js"
import { ObservationEvaluator } from "./observation-evaluator.js"

/**
 * Creates a memory plugin for an agent.
 * All agents sharing the same MemoryManager instance share a memory pool,
 * but searches are scoped by agentName so each agent sees their own memories.
 *
 * Provides:
 * - MemoryProvider: auto-injects relevant memories as context before each LLM step
 * - ObservationEvaluator: auto-stores task results after each run
 * - memory_search action: explicit tool agents can call to search memory
 * - memory_recent action: get recent memories regardless of query
 */
export function createMemoryPlugin(manager: MemoryManager): Plugin {
	return {
		name: "memory",
		description: "Gives agents persistent memory across tasks using BM25 search",
		providers: [new MemoryProvider(manager)],
		evaluators: [new ObservationEvaluator(manager)],
		actions: [
			{
				name: "memory_search",
				description: "Search your memory for past tasks, observations, and facts relevant to a query. Use this to recall what you have done before.",
				parametersSchema: {
					type: "object",
					properties: {
						query: { type: "string", description: "What to search for in memory" },
					},
					required: ["query"],
				},
				handler: async (_runtime: IAgentRuntime, _message: Message, args: Record<string, unknown>) => {
					const query = args.query as string
					// Search the shared swarm memory pool — any agent's memories
					const results = manager.search(query, { limit: 8 })
					if (results.length === 0) {
						return { status: "success" as const, data: "No relevant memories found.", durationMs: 0 }
					}
					const text = results
						.map((r) => {
							const age = Math.floor((Date.now() - r.createdAt) / 60000)
							const ageStr = age < 60 ? `${age}m ago` : `${Math.floor(age / 60)}h ago`
							return `[${r.agentName}/${r.type}] ${r.content} (${ageStr})`
						})
						.join("\n")
					return { status: "success" as const, data: text, durationMs: 0 }
				},
			},
			{
				name: "memory_recent",
				description: "Get your most recent memories — past tasks completed and observations made.",
				parametersSchema: {
					type: "object",
					properties: {
						limit: { type: "number", description: "Number of recent memories to retrieve (default 5)" },
					},
					required: [],
				},
				handler: async (_runtime: IAgentRuntime, _message: Message, args: Record<string, unknown>) => {
					const limit = (args.limit as number | undefined) ?? 5
					// Get recent memories from the shared swarm pool
					const results = manager.getRecent({ limit })
					if (results.length === 0) {
						return { status: "success" as const, data: "No memories yet.", durationMs: 0 }
					}
					const text = results
						.map((r) => {
							const age = Math.floor((Date.now() - r.createdAt) / 60000)
							const ageStr = age < 60 ? `${age}m ago` : `${Math.floor(age / 60)}h ago`
							return `[${r.agentName}/${r.type}] ${r.content} (${ageStr})`
						})
						.join("\n")
					return { status: "success" as const, data: text, durationMs: 0 }
				},
			},
		],
		init: async () => {
			manager.load()
		},
		cleanup: async () => {
			manager.save()
		},
	}
}
