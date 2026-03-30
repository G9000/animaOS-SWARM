import {
	AgentRuntime,
	EventBus,
	OpenAIAdapter,
	type AgentConfig,
	type IModelAdapter,
	type TaskResult,
} from "@animaOS-SWARM/core"
import { SwarmCoordinator } from "@animaOS-SWARM/swarm"
import type { SwarmConfig } from "@animaOS-SWARM/swarm"
import { TaskHistory, DocumentStore } from "@animaOS-SWARM/memory"

export class AppState {
	readonly eventBus = new EventBus()
	readonly agents = new Map<string, AgentRuntime>()
	readonly swarms = new Map<string, SwarmCoordinator>()
	readonly taskHistory = new TaskHistory()
	readonly documentStore = new DocumentStore()
	private modelAdapter: IModelAdapter

	constructor() {
		this.modelAdapter = new OpenAIAdapter()
	}

	getModelAdapter(): IModelAdapter {
		return this.modelAdapter
	}

	async createAgent(config: AgentConfig): Promise<AgentRuntime> {
		const runtime = new AgentRuntime({
			config,
			modelAdapter: this.modelAdapter,
			eventBus: this.eventBus,
		})
		await runtime.init()
		this.agents.set(runtime.agentId, runtime)
		return runtime
	}

	async runAgent(agentId: string, task: string): Promise<TaskResult> {
		const agent = this.agents.get(agentId)
		if (!agent) throw new Error(`Agent ${agentId} not found`)

		const result = await agent.run(task)

		this.taskHistory.record({
			id: `${agentId}-${Date.now()}`,
			agentId,
			task,
			result: result.status === "success" ? JSON.stringify(result.data) : (result.error ?? ""),
			status: result.status,
			timestamp: Date.now(),
			durationMs: result.durationMs,
			tokensUsed: agent.getState().tokenUsage.totalTokens,
		})

		return result
	}

	async deleteAgent(agentId: string): Promise<void> {
		const agent = this.agents.get(agentId)
		if (agent) {
			await agent.stop()
			this.agents.delete(agentId)
		}
	}

	createSwarm(config: SwarmConfig): SwarmCoordinator {
		const coordinator = new SwarmCoordinator(config, this.modelAdapter, this.eventBus)
		this.swarms.set(coordinator.id, coordinator)
		return coordinator
	}
}
