import OpenAI from "openai"
import type {
	IModelAdapter,
	ModelConfig,
	GenerateOptions,
	GenerateResult,
	ToolCall,
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
}
