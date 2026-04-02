import type {
	Action,
	AgentConfig,
	AgentState,
	Content,
	Plugin,
	TaskResult,
} from "@animaOS-SWARM/core"

import type { DaemonClient } from "./client.js"

export interface AgentSnapshot {
	state: AgentState
	messageCount: number
	eventCount: number
	lastTask: TaskResult<Content> | null
}

export interface AgentMemory {
	id: string
	agentId: string
	agentName: string
	type: string
	content: string
	importance: number
	createdAt: number
	tags?: string[] | null
}

export interface AgentRunResponse {
	agent: AgentSnapshot
	result: TaskResult<Content>
}

export function agent<T extends AgentConfig>(config: T): T {
	return config
}

export function plugin<T extends Plugin>(config: T): T {
	return config
}

export function action<T extends Action>(config: T): T {
	return config
}

export class AgentsClient {
	constructor(private readonly client: DaemonClient) {}

	async create(config: AgentConfig): Promise<AgentSnapshot> {
		const response = await this.client.requestJson<{ agent: AgentSnapshot }>("/api/agents", {
			method: "POST",
			body: config,
		})

		return response.agent
	}

	async list(): Promise<AgentSnapshot[]> {
		const response = await this.client.requestJson<{ agents: AgentSnapshot[] }>("/api/agents")
		return response.agents
	}

	async get(agentId: string): Promise<AgentSnapshot> {
		const response = await this.client.requestJson<{ agent: AgentSnapshot }>(
			`/api/agents/${agentId}`,
		)
		return response.agent
	}

	async run(agentId: string, input: Content): Promise<AgentRunResponse> {
		return this.client.requestJson<AgentRunResponse>(`/api/agents/${agentId}/run`, {
			method: "POST",
			body: input,
		})
	}

	async recentMemories(
		agentId: string,
		options: {
			limit?: number
		} = {},
	): Promise<AgentMemory[]> {
		const search = new URLSearchParams()
		if (options.limit !== undefined) {
			search.set("limit", String(options.limit))
		}

		const path = search.size
			? `/api/agents/${agentId}/memories/recent?${search.toString()}`
			: `/api/agents/${agentId}/memories/recent`

		const response = await this.client.requestJson<{ memories: AgentMemory[] }>(path)
		return response.memories
	}
}
