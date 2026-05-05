import Anthropic from "@anthropic-ai/sdk"
import type {
	IModelAdapter,
	ModelConfig,
	GenerateOptions,
	GenerateResult,
	ToolCall,
	StreamChunk,
	Action,
} from "../types/index.js"

function actionsToTools(actions: Action[]): Anthropic.Tool[] {
	return actions.map((a) => ({
		name: a.name,
		description: a.description,
		input_schema: a.parametersSchema as Anthropic.Tool.InputSchema,
	}))
}

/**
 * Map internal messages to the Anthropic Messages API format.
 *
 * Claude only accepts `user` and `assistant` roles. System messages are
 * passed separately. Tool results must be sent as user messages with
 * `tool_result` content blocks.
 */
function mapMessages(
	messages: GenerateOptions["messages"],
): Anthropic.MessageParam[] {
	const mapped: Anthropic.MessageParam[] = []

	for (const msg of messages) {
		if (msg.role === "system") {
			// System messages are handled via the top-level `system` param.
			// If one sneaks into the message list, inject it as a user message.
			mapped.push({ role: "user", content: msg.content.text })
			continue
		}

		if (msg.role === "tool") {
			// Tool results go inside a user message with a tool_result block.
			// We need a tool_use_id — stored in content.metadata by convention.
			const toolUseId = (msg.content.metadata?.toolCallId as string) ?? msg.id
			mapped.push({
				role: "user",
				content: [
					{
						type: "tool_result",
						tool_use_id: toolUseId,
						content: msg.content.text,
					},
				],
			})
			continue
		}

		// user / assistant — straightforward mapping
		mapped.push({
			role: msg.role as "user" | "assistant",
			content: msg.content.text,
		})
	}

	// Claude requires messages to start with a user turn. If the first message
	// is an assistant message, prepend an empty user message.
	if (mapped.length > 0 && mapped[0].role === "assistant") {
		mapped.unshift({ role: "user", content: "" })
	}

	// Claude disallows consecutive messages with the same role.
	// Merge adjacent same-role messages into a single message.
	const merged: Anthropic.MessageParam[] = []
	for (const m of mapped) {
		const prev = merged[merged.length - 1]
		if (prev && prev.role === m.role) {
			// Both are same role — combine their content
			const prevText = typeof prev.content === "string" ? prev.content : ""
			const curText = typeof m.content === "string" ? m.content : ""
			if (prevText !== undefined && curText !== undefined) {
				prev.content = prevText + "\n" + curText
			}
		} else {
			merged.push(m)
		}
	}

	return merged
}

export class AnthropicAdapter implements IModelAdapter {
	provider = "anthropic" as const
	private client: Anthropic

	constructor(apiKey?: string, baseUrl?: string) {
		this.client = new Anthropic({
			apiKey: apiKey ?? process.env.ANTHROPIC_API_KEY,
			...(baseUrl ? { baseURL: baseUrl } : {}),
		})
	}

	async generate(config: ModelConfig, options: GenerateOptions): Promise<GenerateResult> {
		const messages = mapMessages(options.messages)
		const tools = options.actions && options.actions.length > 0
			? actionsToTools(options.actions)
			: undefined

		const response = await this.client.messages.create({
			model: config.model,
			system: options.system,
			messages,
			...(tools && tools.length > 0 ? { tools } : {}),
			temperature: options.temperature ?? config.temperature,
			max_tokens: options.maxTokens ?? config.maxTokens ?? 4096,
		})

		// Extract text content and tool use blocks from the response
		let textContent = ""
		const toolCalls: ToolCall[] = []

		for (const block of response.content) {
			if (block.type === "text") {
				textContent += block.text
			} else if (block.type === "tool_use") {
				toolCalls.push({
					id: block.id,
					name: block.name,
					args: block.input as Record<string, unknown>,
				})
			}
		}

		const stopReason: GenerateResult["stopReason"] =
			response.stop_reason === "tool_use"
				? "tool_call"
				: response.stop_reason === "max_tokens"
					? "max_tokens"
					: "end"

		return {
			content: { text: textContent },
			toolCalls: toolCalls.length > 0 ? toolCalls : undefined,
			usage: {
				promptTokens: response.usage.input_tokens,
				completionTokens: response.usage.output_tokens,
				totalTokens: response.usage.input_tokens + response.usage.output_tokens,
			},
			stopReason,
		}
	}

	async *generateStream(
		config: ModelConfig,
		options: GenerateOptions,
	): AsyncGenerator<StreamChunk> {
		const messages = mapMessages(options.messages)
		const tools = options.actions && options.actions.length > 0
			? actionsToTools(options.actions)
			: undefined

		const stream = this.client.messages.stream({
			model: config.model,
			system: options.system,
			messages,
			...(tools && tools.length > 0 ? { tools } : {}),
			temperature: options.temperature ?? config.temperature,
			max_tokens: options.maxTokens ?? config.maxTokens ?? 4096,
		})

		// Track tool_use blocks being assembled across events
		let currentToolId = ""
		let currentToolName = ""
		let currentToolArgs = ""

		for await (const event of stream) {
			if (event.type === "content_block_start") {
				if (event.content_block.type === "text") {
					// Text block starting — nothing to emit yet
				} else if (event.content_block.type === "tool_use") {
					currentToolId = event.content_block.id
					currentToolName = event.content_block.name
					currentToolArgs = ""
				}
			} else if (event.type === "content_block_delta") {
				if (event.delta.type === "text_delta") {
					yield { type: "text", content: event.delta.text }
				} else if (event.delta.type === "input_json_delta") {
					currentToolArgs += event.delta.partial_json
				}
			} else if (event.type === "content_block_stop") {
				if (currentToolId) {
					let args: Record<string, unknown> = {}
					try {
						args = currentToolArgs ? JSON.parse(currentToolArgs) : {}
					} catch {
						// Partial JSON — best-effort
					}
					yield {
						type: "tool_call",
						toolCall: {
							id: currentToolId,
							name: currentToolName,
							args,
						},
					}
					currentToolId = ""
					currentToolName = ""
					currentToolArgs = ""
				}
			} else if (event.type === "message_stop") {
				yield { type: "done" }
			}
		}
	}
}
