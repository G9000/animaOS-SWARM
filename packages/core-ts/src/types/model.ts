import type { Content, Message } from "./primitives.js"
import type { Action } from "./components.js"

export type ModelProvider = "openai" | "anthropic" | "ollama" | "openrouter" | string

export interface ModelConfig {
	provider: ModelProvider
	model: string
	apiKey?: string
	baseUrl?: string
	temperature?: number
	maxTokens?: number
}

export interface GenerateOptions {
	system: string
	messages: Message[]
	actions?: Action[]
	temperature?: number
	maxTokens?: number
}

export interface GenerateResult {
	content: Content
	toolCalls?: ToolCall[]
	usage: {
		promptTokens: number
		completionTokens: number
		totalTokens: number
	}
	stopReason: "end" | "tool_call" | "max_tokens"
}

export interface ToolCall {
	id: string
	name: string
	args: Record<string, unknown>
}

/**
 * Model adapter interface — abstracts LLM provider calls.
 * Each provider (OpenAI, Anthropic, Ollama) implements this.
 */
export interface IModelAdapter {
	provider: ModelProvider
	generate(config: ModelConfig, options: GenerateOptions): Promise<GenerateResult>
	generateStream?(
		config: ModelConfig,
		options: GenerateOptions,
	): AsyncGenerator<StreamChunk>
}

export interface StreamChunk {
	type: "text" | "tool_call" | "done"
	content?: string
	toolCall?: ToolCall
}
