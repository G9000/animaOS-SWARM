import { randomUUID } from "node:crypto"
import {
	AgentRuntime,
	EventBus,
	type AgentConfig,
	type IModelAdapter,
	type IEventBus,
	type TaskResult,
	type UUID,
} from "@animaOS-SWARM/core"
import { MessageBus } from "./message-bus.js"
import type { SwarmConfig, SwarmState, StrategyContext } from "./types.js"
import { supervisorStrategy } from "./strategies/supervisor.js"
import { dynamicStrategy } from "./strategies/dynamic.js"
import { roundRobinStrategy } from "./strategies/round-robin.js"

export class SwarmCoordinator {
	readonly id: UUID
	private config: SwarmConfig
	private modelAdapter: IModelAdapter
	private eventBus: IEventBus
	private messageBus: MessageBus
	private agents = new Map<string, AgentRuntime>()
	private state: SwarmState

	constructor(config: SwarmConfig, modelAdapter: IModelAdapter, eventBus?: IEventBus) {
		this.id = randomUUID() as UUID
		this.config = config
		this.modelAdapter = modelAdapter
		this.eventBus = eventBus ?? new EventBus()
		this.messageBus = new MessageBus()

		this.state = {
			id: this.id,
			status: "idle",
			agentIds: [],
			results: [],
			tokenUsage: { promptTokens: 0, completionTokens: 0, totalTokens: 0 },
		}
	}

	async run(task: string): Promise<TaskResult> {
		this.state.status = "running"
		this.state.startedAt = Date.now()
		await this.eventBus.emit("swarm:created", { swarmId: this.id, strategy: this.config.strategy })

		const strategyFn = this.getStrategy()

		const ctx: StrategyContext = {
			task,
			managerConfig: this.config.manager,
			workerConfigs: this.config.workers,
			spawnAgent: (config) => this.spawnAgent(config),
			messageBus: this.messageBus,
			maxTurns: this.config.maxTurns ?? this.config.workers.length + 1,
		}

		try {
			const result = await strategyFn(ctx)

			this.state.status = "completed"
			this.state.completedAt = Date.now()
			this.state.results.push(result)
			this.aggregateTokenUsage()

			await this.eventBus.emit("swarm:completed", { swarmId: this.id, result })
			return result
		} catch (err) {
			this.state.status = "failed"
			this.state.completedAt = Date.now()

			const result: TaskResult = {
				status: "error",
				error: err instanceof Error ? err.message : String(err),
				durationMs: Date.now() - (this.state.startedAt ?? Date.now()),
			}
			return result
		} finally {
			await this.terminateAll()
		}
	}

	private async spawnAgent(config: AgentConfig): Promise<{ id: string; run: (input: string) => Promise<TaskResult> }> {
		const maxAgents = this.config.maxConcurrentAgents ?? 20
		if (this.agents.size >= maxAgents) {
			throw new Error(`Max concurrent agents (${maxAgents}) reached`)
		}

		const runtime = new AgentRuntime({
			config,
			modelAdapter: this.modelAdapter,
			eventBus: this.eventBus,
			onSend: async (targetId, message) => {
				this.messageBus.send(runtime.agentId, targetId, message)
			},
			onSpawn: async (spawnConfig) => {
				const child = await this.spawnAgent(spawnConfig)
				if (spawnConfig.task) {
					return child.run(spawnConfig.task)
				}
				return { status: "success", data: { agentId: child.id }, durationMs: 0 }
			},
			onBroadcast: async (message) => {
				this.messageBus.broadcast(runtime.agentId, message)
			},
		})

		await runtime.init()
		this.agents.set(runtime.agentId, runtime)
		this.state.agentIds.push(runtime.agentId)
		this.messageBus.registerAgent(runtime.agentId)

		return {
			id: runtime.agentId,
			run: (input: string) => runtime.run(input),
		}
	}

	async terminate(agentId: string): Promise<void> {
		const agent = this.agents.get(agentId)
		if (agent) {
			await agent.stop()
			this.agents.delete(agentId)
			this.messageBus.unregisterAgent(agentId)
		}
	}

	private async terminateAll(): Promise<void> {
		const ids = [...this.agents.keys()]
		for (const id of ids) {
			await this.terminate(id)
		}
	}

	private aggregateTokenUsage(): void {
		let prompt = 0, completion = 0, total = 0
		for (const agent of this.agents.values()) {
			const s = agent.getState()
			prompt += s.tokenUsage.promptTokens
			completion += s.tokenUsage.completionTokens
			total += s.tokenUsage.totalTokens
		}
		this.state.tokenUsage = { promptTokens: prompt, completionTokens: completion, totalTokens: total }
	}

	private getStrategy() {
		switch (this.config.strategy) {
			case "supervisor": return supervisorStrategy
			case "dynamic": return dynamicStrategy
			case "round-robin": return roundRobinStrategy
			default: throw new Error(`Unknown strategy: ${this.config.strategy}`)
		}
	}

	getState(): SwarmState {
		this.aggregateTokenUsage()
		return { ...this.state }
	}

	getMessageBus(): MessageBus {
		return this.messageBus
	}
}
