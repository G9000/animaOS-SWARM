import { randomUUID } from "node:crypto"
import type {
	UUID,
	Content,
	Message,
	TaskResult,
	AgentConfig,
	AgentState,
	IAgentRuntime,
	Action,
	Provider,
	Evaluator,
	Plugin,
	IModelAdapter,
	ModelConfig,
	GenerateOptions,
	ToolCall,
	IEventBus,
} from "../types/index.js"

export interface AgentRuntimeOptions {
	config: AgentConfig
	modelAdapter: IModelAdapter
	eventBus: IEventBus
	/** Callback for sending messages to other agents via the coordinator */
	onSend?: (targetAgentId: string, message: Content) => Promise<void>
	/** Callback for spawning child agents via the coordinator */
	onSpawn?: (config: AgentConfig & { task?: string }) => Promise<TaskResult>
	/** Callback for broadcasting to the swarm */
	onBroadcast?: (message: Content) => Promise<void>
}

export class AgentRuntime implements IAgentRuntime {
	readonly agentId: UUID
	readonly config: AgentConfig

	private state: AgentState
	private actions: Map<string, Action> = new Map()
	private providers: Provider[] = []
	private evaluators: Evaluator[] = []
	private plugins: Plugin[] = []
	private modelAdapter: IModelAdapter
	private eventBus: IEventBus
	private onSend?: AgentRuntimeOptions["onSend"]
	private onSpawn?: AgentRuntimeOptions["onSpawn"]
	private onBroadcast?: AgentRuntimeOptions["onBroadcast"]

	constructor(options: AgentRuntimeOptions) {
		this.agentId = randomUUID() as UUID
		this.config = options.config
		this.modelAdapter = options.modelAdapter
		this.eventBus = options.eventBus
		this.onSend = options.onSend
		this.onSpawn = options.onSpawn
		this.onBroadcast = options.onBroadcast

		this.state = {
			id: this.agentId,
			name: options.config.name,
			status: "idle",
			config: options.config,
			createdAt: Date.now(),
			tokenUsage: { promptTokens: 0, completionTokens: 0, totalTokens: 0 },
		}

		// Register tools from config
		if (options.config.tools) {
			for (const action of options.config.tools) {
				this.actions.set(action.name, action)
			}
		}

		// Register plugins from config
		if (options.config.plugins) {
			for (const plugin of options.config.plugins) {
				this.registerPluginSync(plugin)
			}
		}
	}

	async init(): Promise<void> {
		for (const plugin of this.plugins) {
			if (plugin.init) {
				await plugin.init(this)
			}
		}
		await this.eventBus.emit("agent:spawned", { agentId: this.agentId, name: this.config.name }, this.agentId)
	}

	async run(input: string | Content): Promise<TaskResult> {
		const startTime = Date.now()
		this.state.status = "running"
		await this.eventBus.emit("task:started", { agentId: this.agentId }, this.agentId)

		const content: Content = typeof input === "string" ? { text: input } : input

		const userMessage: Message = {
			id: randomUUID() as UUID,
			agentId: this.agentId,
			roomId: randomUUID() as UUID,
			content,
			role: "user",
			createdAt: Date.now(),
		}

		try {
			// Build context from providers
			const contextParts: string[] = []
			for (const provider of this.providers) {
				const result = await provider.get(this, userMessage)
				contextParts.push(`[${provider.name}]: ${result.text}`)
			}

			// Build system prompt
			const systemParts: string[] = []
			if (this.config.bio) {
				systemParts.push(`## Who You Are\n${this.config.bio}`)
			}
			if (this.config.lore) {
				systemParts.push(`## Your Backstory\n${this.config.lore}`)
			}
			if (this.config.adjectives && this.config.adjectives.length > 0) {
				systemParts.push(`## Your Personality\nYou are ${this.config.adjectives.join(", ")}.`)
			}
			if (this.config.topics && this.config.topics.length > 0) {
				systemParts.push(`## Your Expertise\nYou specialize in: ${this.config.topics.join(", ")}.`)
			}
			if (this.config.knowledge && this.config.knowledge.length > 0) {
				systemParts.push(`## What You Know\n${this.config.knowledge.map((k) => `- ${k}`).join("\n")}`)
			}
			if (this.config.style) {
				systemParts.push(`## How You Communicate\n${this.config.style}`)
			}
			systemParts.push(this.config.system ?? "You are a helpful task agent.")
			if (contextParts.length > 0) {
				systemParts.push("\n## Context\n" + contextParts.join("\n"))
			}

			const messages: Message[] = [userMessage]
			const availableActions = this.getAvailableActions()

			// Agent loop — call LLM, execute tools, repeat until done
			let result = await this.step(systemParts.join("\n"), messages, availableActions)
			let iterations = 0
			const maxIterations = 20

			while (result.stopReason === "tool_call" && result.toolCalls && iterations < maxIterations) {
				iterations++

				// Add the assistant's response (with tool calls) to the conversation
				messages.push({
					id: randomUUID() as UUID,
					agentId: this.agentId,
					roomId: userMessage.roomId,
					content: {
						text: result.content.text ?? "",
						metadata: { toolCalls: result.toolCalls },
					},
					role: "assistant",
					createdAt: Date.now(),
				})

				for (const toolCall of result.toolCalls) {
					const toolResult = await this.executeTool(toolCall, userMessage)

					messages.push({
						id: randomUUID() as UUID,
						agentId: this.agentId,
						roomId: userMessage.roomId,
						content: {
							text: JSON.stringify(toolResult),
							metadata: { toolCallId: toolCall.id },
						},
						role: "tool",
						createdAt: Date.now(),
					})
				}

				result = await this.step(systemParts.join("\n"), messages, availableActions)
			}

			// Run evaluators
			for (const evaluator of this.evaluators) {
				const shouldRun = await evaluator.validate(this, userMessage)
				if (shouldRun) {
					await evaluator.handler(this, userMessage, result.content)
				}
			}

			this.state.status = "completed"
			const taskResult: TaskResult = {
				status: "success",
				data: result.content,
				durationMs: Date.now() - startTime,
			}

			await this.eventBus.emit("task:completed", { agentId: this.agentId, result: taskResult }, this.agentId)
			return taskResult
		} catch (err) {
			this.state.status = "failed"
			const taskResult: TaskResult = {
				status: "error",
				error: err instanceof Error ? err.message : String(err),
				durationMs: Date.now() - startTime,
			}

			await this.eventBus.emit("task:failed", { agentId: this.agentId, error: taskResult.error }, this.agentId)
			return taskResult
		}
	}

	private async step(system: string, messages: Message[], actions: Action[]) {
		const modelConfig: ModelConfig = {
			provider: this.config.provider ?? "openai",
			model: this.config.model,
			...(this.config.settings ?? {}),
		}

		const options: GenerateOptions = {
			system,
			messages,
			actions,
			temperature: this.config.settings?.temperature as number | undefined,
			maxTokens: this.config.settings?.maxTokens as number | undefined,
		}

		const result = await this.modelAdapter.generate(modelConfig, options)

		// Track token usage
		this.state.tokenUsage.promptTokens += result.usage.promptTokens
		this.state.tokenUsage.completionTokens += result.usage.completionTokens
		this.state.tokenUsage.totalTokens += result.usage.totalTokens

		await this.eventBus.emit("agent:tokens", {
			agentId: this.agentId,
			usage: { ...this.state.tokenUsage },
		}, this.agentId)

		return result
	}

	private async executeTool(toolCall: ToolCall, message: Message): Promise<TaskResult> {
		const action = this.actions.get(toolCall.name)
		if (!action) {
			return { status: "error", error: `Unknown tool: ${toolCall.name}`, durationMs: 0 }
		}

		await this.eventBus.emit("tool:before", {
			agentId: this.agentId,
			toolName: toolCall.name,
			args: toolCall.args,
		}, this.agentId)

		const startTime = Date.now()
		try {
			const result = await action.handler(this, message, toolCall.args)

			await this.eventBus.emit("tool:after", {
				agentId: this.agentId,
				toolName: toolCall.name,
				status: result.status,
				durationMs: Date.now() - startTime,
			}, this.agentId)

			return result
		} catch (err) {
			const durationMs = Date.now() - startTime
			await this.eventBus.emit("tool:after", {
				agentId: this.agentId,
				toolName: toolCall.name,
				status: "error",
				durationMs,
			}, this.agentId)

			return { status: "error", error: err instanceof Error ? err.message : String(err), durationMs }
		}
	}

	private getAvailableActions(): Action[] {
		return Array.from(this.actions.values())
	}

	getActions(): Action[] {
		return this.getAvailableActions()
	}

	registerPlugin(plugin: Plugin): void {
		this.registerPluginSync(plugin)
	}

	private registerPluginSync(plugin: Plugin): void {
		this.plugins.push(plugin)
		if (plugin.actions) {
			for (const action of plugin.actions) {
				this.actions.set(action.name, action)
			}
		}
		if (plugin.providers) {
			this.providers.push(...plugin.providers)
		}
		if (plugin.evaluators) {
			this.evaluators.push(...plugin.evaluators)
		}
	}

	async send(targetAgentId: string, message: Content): Promise<void> {
		if (!this.onSend) throw new Error("Agent is not connected to a swarm coordinator")
		await this.onSend(targetAgentId, message)
		await this.eventBus.emit("agent:message", {
			from: this.agentId,
			to: targetAgentId,
			message,
		}, this.agentId)
	}

	async spawn(config: AgentConfig & { task?: string }): Promise<TaskResult> {
		if (!this.onSpawn) throw new Error("Agent is not connected to a swarm coordinator")
		return this.onSpawn(config)
	}

	async broadcast(message: Content): Promise<void> {
		if (!this.onBroadcast) throw new Error("Agent is not connected to a swarm coordinator")
		await this.onBroadcast(message)
	}

	async stop(): Promise<void> {
		this.state.status = "terminated"
		for (const plugin of this.plugins) {
			if (plugin.cleanup) {
				await plugin.cleanup(this)
			}
		}
		await this.eventBus.emit("agent:terminated", { agentId: this.agentId }, this.agentId)
	}

	getState(): AgentState {
		return { ...this.state }
	}
}
