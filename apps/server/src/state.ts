import {
  AgentRuntime,
  EventBus,
  OpenAIAdapter,
  type AgentConfig,
  type IModelAdapter,
  type TaskResult,
} from '@animaOS-SWARM/core';
import { SwarmCoordinator } from '@animaOS-SWARM/swarm';
import type { SwarmConfig } from '@animaOS-SWARM/swarm';
import { TaskHistory, DocumentStore } from '@animaOS-SWARM/memory';
import { MockModelAdapter } from './mock-model-adapter.js';

export class AppState {
  readonly eventBus = new EventBus();
  readonly agents = new Map<string, AgentRuntime>();
  readonly swarms = new Map<string, SwarmCoordinator>();
  readonly taskHistory = new TaskHistory();
  readonly documentStore = new DocumentStore();
  private modelAdapter: IModelAdapter | null = null;

  getModelAdapter(): IModelAdapter {
    if (!this.modelAdapter) {
      this.modelAdapter =
        process.env.ANIMA_MODEL_ADAPTER === 'mock'
          ? new MockModelAdapter()
          : new OpenAIAdapter();
    }

    return this.modelAdapter;
  }

  async createAgent(config: AgentConfig): Promise<AgentRuntime> {
    const modelAdapter = this.getModelAdapter();
    const runtime = new AgentRuntime({
      config,
      modelAdapter,
      eventBus: this.eventBus,
    });
    await runtime.init();
    this.agents.set(runtime.agentId, runtime);
    return runtime;
  }

  async runAgent(agentId: string, task: string): Promise<TaskResult> {
    const agent = this.agents.get(agentId);
    if (!agent) throw new Error(`Agent ${agentId} not found`);

    const result = await agent.run(task);

    this.taskHistory.record({
      id: `${agentId}-${Date.now()}`,
      agentId,
      task,
      result:
        result.status === 'success'
          ? JSON.stringify(result.data)
          : result.error ?? '',
      status: result.status,
      timestamp: Date.now(),
      durationMs: result.durationMs,
      tokensUsed: agent.getState().tokenUsage.totalTokens,
    });

    return result;
  }

  async deleteAgent(agentId: string): Promise<void> {
    const agent = this.agents.get(agentId);
    if (agent) {
      await agent.stop();
      this.agents.delete(agentId);
    }
  }

  async createSwarm(config: SwarmConfig): Promise<SwarmCoordinator> {
    const coordinator = new SwarmCoordinator(
      config,
      this.getModelAdapter(),
      this.eventBus
    );
    this.swarms.set(coordinator.id, coordinator);

    await this.eventBus.emit('swarm:created', {
      swarmId: coordinator.id,
      strategy: config.strategy,
    });

    return coordinator;
  }
}
