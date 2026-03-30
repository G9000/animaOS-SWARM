import OpenAI from "openai"
import type {
	IModelAdapter,
	ModelConfig,
	GenerateOptions,
	GenerateResult,
	ToolCall,
	StreamChunk,
	Action,
} from "../types/index.js"

function actionsToTools(actions: Action[]): OpenAI.Chat.Completions.ChatCompletionTool[] {
	return actions.map((a) => ({
		type: "function" as const,
		function: {
			name: a.name,
			description: a.description,
			parameters: a.parameters,
		},
	}))
}

export class OpenAIAdapter implements IModelAdapter {
	provider = "openai" as const
	private client: OpenAI

	constructor(apiKey?: string, baseUrl?: string) {
		this.client = new OpenAI({
			apiKey: apiKey ?? process.env.OPENAI_API_KEY,
			baseURL: baseUrl,
		})
	}

	async generate(config: ModelConfig, options: GenerateOptions): Promise<GenerateResult> {
		const messages: OpenAI.Chat.Completions.ChatCompletionMessageParam[] = [
			{ role: "system", content: options.system },
		]

		for (const msg of options.messages) {
			if (msg.role === "assistant" && msg.content.metadata?.toolCalls) {
				// Assistant message that requested tool calls
				const toolCalls = msg.content.metadata.toolCalls as ToolCall[]
				messages.push({
					role: "assistant",
					content: msg.content.text || null,
					tool_calls: toolCalls.map((tc) => ({
						id: tc.id,
						type: "function" as const,
						function: {
							name: tc.name,
							arguments: JSON.stringify(tc.args),
						},
					})),
				})
			} else if (msg.role === "tool") {
				// Tool result — must reference the tool_call_id
				const toolCallId = (msg.content.metadata?.toolCallId as string) ?? msg.id
				messages.push({
					role: "tool",
					tool_call_id: toolCallId,
					content: msg.content.text,
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

		const response = await this.client.chat.completions.create({
			model: config.model,
			messages,
			tools,
			temperature: options.temperature ?? config.temperature,
			max_tokens: options.maxTokens ?? config.maxTokens,
		})

		const choice = response.choices[0]
		const toolCalls: ToolCall[] | undefined = choice.message.tool_calls?.map((tc: any) => ({
			id: tc.id,
			name: (tc.function ?? tc).name,
			args: JSON.parse((tc.function ?? tc).arguments),
		}))

		return {
			content: { text: choice.message.content ?? "" },
			toolCalls,
			usage: {
				promptTokens: response.usage?.prompt_tokens ?? 0,
				completionTokens: response.usage?.completion_tokens ?? 0,
				totalTokens: response.usage?.total_tokens ?? 0,
			},
			stopReason: toolCalls ? "tool_call" : choice.finish_reason === "length" ? "max_tokens" : "end",
		}
	}

	async *generateStream(
		config: ModelConfig,
		options: GenerateOptions,
	): AsyncGenerator<StreamChunk> {
		const messages: OpenAI.Chat.Completions.ChatCompletionMessageParam[] = [
			{ role: "system", content: options.system },
		]

		for (const msg of options.messages) {
			if (msg.role === "assistant" && msg.content.metadata?.toolCalls) {
				// Assistant message that requested tool calls
				const toolCalls = msg.content.metadata.toolCalls as ToolCall[]
				messages.push({
					role: "assistant",
					content: msg.content.text || null,
					tool_calls: toolCalls.map((tc) => ({
						id: tc.id,
						type: "function" as const,
						function: {
							name: tc.name,
							arguments: JSON.stringify(tc.args),
						},
					})),
				})
			} else if (msg.role === "tool") {
				// Tool result — must reference the tool_call_id
				const toolCallId = (msg.content.metadata?.toolCallId as string) ?? msg.id
				messages.push({
					role: "tool",
					tool_call_id: toolCallId,
					content: msg.content.text,
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

		const stream = await this.client.chat.completions.create({
			model: config.model,
			messages,
			tools,
			temperature: options.temperature ?? config.temperature,
			max_tokens: options.maxTokens ?? config.maxTokens,
			stream: true,
		})

		// Track tool call fragments being assembled across deltas.
		// OpenAI sends tool_call id + name in the first delta for a given index,
		// then streams arguments across subsequent deltas.
		const pendingToolCalls = new Map<
			number,
			{ id: string; name: string; args: string }
		>()

		for await (const chunk of stream) {
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
}
