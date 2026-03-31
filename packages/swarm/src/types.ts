import type { AgentConfig, UUID, Content, TaskResult } from "@animaOS-SWARM/core"

export type SwarmStrategy = "supervisor" | "dynamic" | "round-robin"

export interface SwarmConfig {
	strategy: SwarmStrategy
	manager: AgentConfig
	workers: AgentConfig[]
	maxConcurrentAgents?: number
	maxTurns?: number
	tokenBudget?: number
}

export type SwarmStatus = "idle" | "running" | "completed" | "failed"

export interface SwarmState {
	id: UUID
	status: SwarmStatus
	agentIds: string[]
	results: TaskResult[]
	tokenUsage: { promptTokens: number; completionTokens: number; totalTokens: number }
	startedAt?: number
	completedAt?: number
}

export interface AgentMessage {
	id: string
	from: string
	to: string | "broadcast"
	content: Content
	timestamp: number
}

export type StrategyFn = (ctx: StrategyContext) => Promise<TaskResult>

export interface StrategyContext {
	task: string
	managerConfig: AgentConfig
	workerConfigs: AgentConfig[]
	spawnAgent: (config: AgentConfig) => Promise<{ id: string; run: (input: string) => Promise<TaskResult> }>
	messageBus: IMessageBus
	maxTurns: number
}

export interface IMessageBus {
	send(from: string, to: string, content: Content): void
	broadcast(from: string, content: Content): void
	getMessages(agentId: string): AgentMessage[]
	getAllMessages(): AgentMessage[]
	clear(): void
	/** Clear per-agent inboxes between tasks without losing the global message history. */
	clearInboxes(): void
}
