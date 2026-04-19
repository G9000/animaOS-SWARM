import type {
	IModelAdapter,
	ModelConfig,
	GenerateOptions,
	GenerateResult,
	ToolCall,
	StreamChunk,
	Action,
} from "../types/index.js"

const DEFAULT_BASE_URL = "http://127.0.0.1:11434/v1"

/** Map actions to the OpenAI-compatible tool format that Ollama supports. */
function actionsToTools(actions: Action[]) {
	return actions.map((a) => ({
		type: "function" as const,
		function: {
			name: a.name,
			description: a.description,
			parameters: a.parameters,
		},
	}))
}

interface OllamaMessage {
	role: "system" | "user" | "assistant" | "tool"
	content: string
	tool_call_id?: string
	tool_calls?: Array<{
		id: string
		type: "function"
		function: { name: string; arguments: string }
	}>
}

interface OllamaChatResponse {
	id: string
	choices: Array<{
		index: number
		message: {
			role: string
			content: string | null
			tool_calls?: Array<{
				id: string
				type: "function"
				function: { name: string; arguments: string }
			}>
		}
		finish_reason: string
	}>
	usage?: {
		prompt_tokens: number
		completion_tokens: number
		total_tokens: number
	}
}

interface OllamaStreamDelta {
	id: string
	choices: Array<{
		index: number
		delta: {
			role?: string
			content?: string | null
			tool_calls?: Array<{
				index: number
				id?: string
				type?: "function"
				function?: { name?: string; arguments?: string }
			}>
		}
		finish_reason: string | null
	}>
	usage?: {
		prompt_tokens: number
		completion_tokens: number
		total_tokens: number
	}
}

export class OllamaAdapter implements IModelAdapter {
	provider = "ollama" as const
	private baseUrl: string

	constructor(baseUrl?: string) {
		this.baseUrl = baseUrl ?? DEFAULT_BASE_URL
	}

	async generate(config: ModelConfig, options: GenerateOptions): Promise<GenerateResult> {
		const messages: OllamaMessage[] = [
			{ role: "system", content: options.system },
		]

		for (const msg of options.messages) {
			if (msg.role === "tool") {
				messages.push({
					role: "tool",
					content: msg.content.text,
					tool_call_id: (msg.content.metadata?.toolCallId as string) ?? msg.id,
				})
			} else {
				messages.push({
					role: msg.role as "user" | "assistant",
					content: msg.content.text,
				})
			}
		}

		const tools = options.actions && options.actions.length > 0
			? actionsToTools(options.actions)
			: undefined

		const body: Record<string, unknown> = {
			model: config.model,
			messages,
			stream: false,
		}

		if (tools && tools.length > 0) {
			body.tools = tools
		}
		if (options.temperature ?? config.temperature) {
			body.temperature = options.temperature ?? config.temperature
		}
		if (options.maxTokens ?? config.maxTokens) {
			body.max_tokens = options.maxTokens ?? config.maxTokens
		}

		const url = `${config.baseUrl ?? this.baseUrl}/chat/completions`
		const response = await fetch(url, {
			method: "POST",
			headers: {
				"Content-Type": "application/json",
				...(config.apiKey ? { Authorization: `Bearer ${config.apiKey}` } : {}),
			},
			body: JSON.stringify(body),
		})

		if (!response.ok) {
			const errorText = await response.text()
			throw new Error(
				`Ollama API error (${response.status}): ${errorText}`,
			)
		}

		const data = (await response.json()) as OllamaChatResponse
		const choice = data.choices[0]

		const toolCalls: ToolCall[] | undefined = choice.message.tool_calls?.map((tc) => ({
			id: tc.id,
			name: tc.function.name,
			args: JSON.parse(tc.function.arguments),
		}))

		return {
			content: { text: choice.message.content ?? "" },
			toolCalls,
			usage: {
				promptTokens: data.usage?.prompt_tokens ?? 0,
				completionTokens: data.usage?.completion_tokens ?? 0,
				totalTokens: data.usage?.total_tokens ?? 0,
			},
			stopReason: toolCalls
				? "tool_call"
				: choice.finish_reason === "length"
					? "max_tokens"
					: "end",
		}
	}

	async *generateStream(
		config: ModelConfig,
		options: GenerateOptions,
	): AsyncGenerator<StreamChunk> {
		const messages: OllamaMessage[] = [
			{ role: "system", content: options.system },
		]

		for (const msg of options.messages) {
			if (msg.role === "tool") {
				messages.push({
					role: "tool",
					content: msg.content.text,
					tool_call_id: (msg.content.metadata?.toolCallId as string) ?? msg.id,
				})
			} else {
				messages.push({
					role: msg.role as "user" | "assistant",
					content: msg.content.text,
				})
			}
		}

		const tools = options.actions && options.actions.length > 0
			? actionsToTools(options.actions)
			: undefined

		const body: Record<string, unknown> = {
			model: config.model,
			messages,
			stream: true,
		}

		if (tools && tools.length > 0) {
			body.tools = tools
		}
		if (options.temperature ?? config.temperature) {
			body.temperature = options.temperature ?? config.temperature
		}
		if (options.maxTokens ?? config.maxTokens) {
			body.max_tokens = options.maxTokens ?? config.maxTokens
		}

		const url = `${config.baseUrl ?? this.baseUrl}/chat/completions`
		const response = await fetch(url, {
			method: "POST",
			headers: {
				"Content-Type": "application/json",
				...(config.apiKey ? { Authorization: `Bearer ${config.apiKey}` } : {}),
			},
			body: JSON.stringify(body),
		})

		if (!response.ok) {
			const errorText = await response.text()
			throw new Error(
				`Ollama API error (${response.status}): ${errorText}`,
			)
		}

		if (!response.body) {
			throw new Error("Ollama streaming response has no body")
		}

		// Track tool call fragments being assembled across SSE chunks
		const pendingToolCalls = new Map<
			number,
			{ id: string; name: string; args: string }
		>()

		const reader = response.body.getReader()
		const decoder = new TextDecoder()
		let buffer = ""

		try {
			while (true) {
				const { done, value } = await reader.read()
				if (done) break

				buffer += decoder.decode(value, { stream: true })
				const lines = buffer.split("\n")
				// Keep the last potentially-incomplete line in the buffer
				buffer = lines.pop() ?? ""

				for (const line of lines) {
					const trimmed = line.trim()
					if (!trimmed || !trimmed.startsWith("data: ")) continue
					const payload = trimmed.slice(6)
					if (payload === "[DONE]") {
						// Flush any pending tool calls
						for (const [, tc] of pendingToolCalls) {
							let args: Record<string, unknown> = {}
							try {
								args = tc.args ? JSON.parse(tc.args) : {}
							} catch {
								// best-effort
							}
							yield {
								type: "tool_call",
								toolCall: { id: tc.id, name: tc.name, args },
							}
						}
						pendingToolCalls.clear()
						yield { type: "done" }
						return
					}

					let chunk: OllamaStreamDelta
					try {
						chunk = JSON.parse(payload) as OllamaStreamDelta
					} catch {
						continue
					}

					const delta = chunk.choices?.[0]?.delta
					if (!delta) continue

					// Text content
					if (delta.content) {
						yield { type: "text", content: delta.content }
					}

					// Tool call deltas (streamed incrementally)
					if (delta.tool_calls) {
						for (const tc of delta.tool_calls) {
							const idx = tc.index
							if (!pendingToolCalls.has(idx)) {
								pendingToolCalls.set(idx, {
									id: tc.id ?? "",
									name: tc.function?.name ?? "",
									args: "",
								})
							}
							const pending = pendingToolCalls.get(idx)!
							if (tc.id) pending.id = tc.id
							if (tc.function?.name) pending.name = tc.function.name
							if (tc.function?.arguments) pending.args += tc.function.arguments
						}
					}

					// Check if the choice is finished
					const finishReason = chunk.choices?.[0]?.finish_reason
					if (finishReason) {
						// Flush pending tool calls
						for (const [, tc] of pendingToolCalls) {
							let args: Record<string, unknown> = {}
							try {
								args = tc.args ? JSON.parse(tc.args) : {}
							} catch {
								// best-effort
							}
							yield {
								type: "tool_call",
								toolCall: { id: tc.id, name: tc.name, args },
							}
						}
						pendingToolCalls.clear()
						yield { type: "done" }
						return
					}
				}
			}
		} finally {
			reader.releaseLock()
		}
	}
}
