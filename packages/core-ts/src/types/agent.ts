import type { UUID, Content, TaskResult } from "./primitives.js"
import type { Action } from "./components.js"
import type { Plugin } from "./plugin.js"

export interface AgentConfig {
	name: string
	model: string
	bio?: string
	lore?: string
	knowledge?: string[]
	topics?: string[]
	adjectives?: string[]
	style?: string
	provider?: string
	system?: string
	tools?: Action[]
	plugins?: Plugin[]
	settings?: AgentSettings
}

export interface AgentSettings {
	temperature?: number
	maxTokens?: number
	timeout?: number
	maxRetries?: number
	[key: string]: unknown
}

export interface AgentState {
	id: UUID
	name: string
	status: AgentStatus
	config: AgentConfig
	createdAt: number
	tokenUsage: TokenUsage
}

export type AgentStatus = "idle" | "running" | "completed" | "failed" | "terminated"

export interface TokenUsage {
	promptTokens: number
	completionTokens: number
	totalTokens: number
}

export interface IAgentRuntime {
	agentId: UUID
	config: AgentConfig

	/** Execute a task and return the result */
	run(input: string | Content): Promise<TaskResult>

	/** Get all registered actions */
	getActions(): Action[]

	/** Register a plugin */
	registerPlugin(plugin: Plugin): void

	/** Send a message to another agent */
	send(targetAgentId: string, message: Content): Promise<void>

	/** Spawn a child agent */
	spawn(config: AgentConfig & { task?: string }): Promise<TaskResult>

	/** Broadcast a message to all agents in the swarm */
	broadcast(message: Content): Promise<void>

	/** Stop the agent */
	stop(): Promise<void>
}
