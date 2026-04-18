import type { IAgentRuntime } from "./agent.js"
import type { Content, Message, TaskResult } from "./primitives.js"

/**
 * Action — something an agent can do (tool call).
 * This is the core primitive for agent capabilities.
 */
export interface Action {
	name: string
	description: string
	parameters: Record<string, unknown>

	/** Validate whether this action should be available given current state */
	validate?: (runtime: IAgentRuntime, message: Message) => Promise<boolean>

	/** Execute the action */
	handler: (
		runtime: IAgentRuntime,
		message: Message,
		args: Record<string, unknown>,
	) => Promise<TaskResult>

	/** Examples of how to use this action */
	examples?: ActionExample[]
}

export interface ActionExample {
	input: string
	args: Record<string, unknown>
	output: string
}

/**
 * Provider — data injected into the agent's context before each LLM call.
 * Providers supply dynamic context like current time, user info, search results.
 */
export interface Provider {
	name: string
	description: string

	/** Generate context data for the current request */
	get: (runtime: IAgentRuntime, message: Message) => Promise<ProviderResult>
}

export interface ProviderResult {
	text: string
	metadata?: Record<string, unknown>
}

/**
 * Evaluator — post-processing logic after each agent response.
 * Used for quality checks, scoring, reflection, follow-up actions.
 */
export interface Evaluator {
	name: string
	description: string

	/** Whether this evaluator should run for the given message */
	validate: (runtime: IAgentRuntime, message: Message) => Promise<boolean>

	/** Evaluate the agent's response */
	handler: (
		runtime: IAgentRuntime,
		message: Message,
		response: Content,
	) => Promise<EvaluatorResult>
}

export interface EvaluatorResult {
	score?: number
	feedback?: string
	followUp?: Content
	metadata?: Record<string, unknown>
}
