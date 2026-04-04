import { randomUUID } from 'node:crypto';
import {
  AgentRuntime,
  EventBus,
  type AgentConfig,
  type IModelAdapter,
  type IEventBus,
  type TaskResult,
  type UUID,
} from '@animaOS-SWARM/core';
import { MessageBus } from './message-bus.js';
import type { SwarmConfig, SwarmState, StrategyContext } from './types.js';
import { supervisorStrategy } from './strategies/supervisor.js';
import { dynamicStrategy } from './strategies/dynamic.js';
import { roundRobinStrategy } from './strategies/round-robin.js';

type AgentHandle = { id: string; run: (input: string) => Promise<TaskResult> };

export class SwarmCoordinator {
  readonly id: UUID;
  private config: SwarmConfig;
  private modelAdapter: IModelAdapter;
  private eventBus: IEventBus;
  private messageBus: MessageBus;
  private agents = new Map<string, AgentRuntime>();
  private state: SwarmState;

  /** Pool of long-running agents keyed by config.name — populated by start() */
  private pool = new Map<string, AgentHandle>();

  /** Serial task chain — ensures only one dispatch runs at a time */
  private taskChain: Promise<unknown> = Promise.resolve();

  constructor(
    config: SwarmConfig,
    modelAdapter: IModelAdapter,
    eventBus?: IEventBus
  ) {
    this.id = randomUUID() as UUID;
    this.config = config;
    this.modelAdapter = modelAdapter;
    this.eventBus = eventBus ?? new EventBus();
    this.messageBus = new MessageBus();

    this.state = {
      id: this.id,
      status: 'idle',
      agentIds: [],
      results: [],
      tokenUsage: { promptTokens: 0, completionTokens: 0, totalTokens: 0 },
    };
  }

  // ─── persistent lifecycle ─────────────────────────────────────────────────

  /**
   * Spawn all configured agents and keep them alive.
   * Call once at startup; agents persist across multiple dispatch() calls.
   */
  async start(): Promise<void> {
    await this.eventBus.emit('swarm:created', {
      swarmId: this.id,
      strategy: this.config.strategy,
    });

    // Pre-spawn workers only. The manager is spawned fresh per task by each strategy
    // so it receives the correct strategy-specific tools (e.g. delegate_task, choose_speaker).
    await Promise.all(
      this.config.workers.map(async (config) => {
        const handle = await this.spawnAgent(config);
        this.pool.set(config.name, handle);
      })
    );

    this.state.status = 'idle';
  }

  /**
   * Dispatch a task to the running agent pool.
   * Tasks are serialised — concurrent calls queue up and run one at a time.
   * Agents are reused across tasks (no spawn/terminate overhead).
   */
  dispatch(task: string): Promise<TaskResult> {
    let resolve!: (r: TaskResult) => void;
    const promise = new Promise<TaskResult>((r) => (resolve = r));

    this.taskChain = this.taskChain
      .catch(() => {}) // don't let a previous task error break the chain
      .then(async () => {
        const result = await this._runTask(task);
        resolve(result);
      });

    return promise;
  }

  /**
   * Gracefully stop all agents.
   * Waits for any running dispatch() to complete first.
   */
  async stop(): Promise<void> {
    await this.taskChain.catch(() => {});
    this.pool.clear();
    await this.terminateAll();
    await this.eventBus.emit('swarm:stopped', { swarmId: this.id });
    this.state.status = 'idle';
  }

  // ─── internal task runner (used by both dispatch() and run()) ─────────────

  private async _runTask(task: string): Promise<TaskResult> {
    this.state.status = 'running';
    this.state.startedAt = Date.now();

    // Clear per-agent inboxes so messages from previous tasks don't bleed in
    this.messageBus.clearInboxes();

    // Re-announce pool agents to the TUI after each reset() clears the display
    for (const [name, handle] of this.pool) {
      await this.eventBus.emit('agent:spawned', { agentId: handle.id, name });
    }

    const strategyFn = this.getStrategy();

    // Pool-aware spawnAgent: returns existing long-running agent if available,
    // otherwise spawns a fresh one (single-shot / run() compatibility)
    const poolAwareSpawn = async (
      config: AgentConfig
    ): Promise<AgentHandle> => {
      const existing = this.pool.get(config.name);
      if (existing) return existing;
      return this.spawnAgent(config);
    };

    const ctx: StrategyContext = {
      task,
      managerConfig: this.config.manager,
      workerConfigs: this.config.workers,
      spawnAgent: poolAwareSpawn,
      messageBus: this.messageBus,
      maxTurns: this.config.maxTurns ?? this.config.workers.length + 1,
    };

    try {
      const result = await strategyFn(ctx);

      this.state.status = 'idle';
      this.state.completedAt = Date.now();
      this.state.results.push(result);
      // Capture token usage while agents are still alive (fixes the post-terminateAll bug)
      this.aggregateTokenUsage();

      // Terminate per-task agents (manager spawned by strategy with strategy-specific tools).
      // Pool agents (workers) are identified by their handle id and must persist.
      const poolIds = new Set([...this.pool.values()].map((h) => h.id));
      for (const agentId of this.agents.keys()) {
        if (!poolIds.has(agentId)) {
          await this.terminate(agentId);
        }
      }

      await this.eventBus.emit('swarm:completed', { swarmId: this.id, result });
      return {
        ...result,
        durationMs: Date.now() - (this.state.startedAt ?? Date.now()),
      };
    } catch (err) {
      this.state.status = 'failed';
      this.state.completedAt = Date.now();

      return {
        status: 'error',
        error: err instanceof Error ? err.message : String(err),
        durationMs: Date.now() - (this.state.startedAt ?? Date.now()),
      };
    }
  }

  // ─── single-shot run() — backward compatible ──────────────────────────────

  /**
   * Single-shot: spawn agents, run task, terminate agents.
   * Kept for backward compatibility. Prefer start() + dispatch() + stop() for
   * interactive/multi-task use (persistent agents).
   */
  async run(task: string): Promise<TaskResult> {
    await this.eventBus.emit('swarm:created', {
      swarmId: this.id,
      strategy: this.config.strategy,
    });

    try {
      return await this._runTask(task);
    } finally {
      await this.terminateAll();
    }
  }

  // ─── agent management ─────────────────────────────────────────────────────

  private async spawnAgent(config: AgentConfig): Promise<AgentHandle> {
    const maxAgents = this.config.maxConcurrentAgents ?? 20;
    if (this.agents.size >= maxAgents) {
      throw new Error(`Max concurrent agents (${maxAgents}) reached`);
    }

    const runtime = new AgentRuntime({
      config,
      modelAdapter: this.modelAdapter,
      eventBus: this.eventBus,
      onSend: async (targetId, message) => {
        this.messageBus.send(runtime.agentId, targetId, message);
      },
      onSpawn: async (spawnConfig) => {
        const child = await this.spawnAgent(spawnConfig);
        if (spawnConfig.task) {
          return child.run(spawnConfig.task);
        }
        return {
          status: 'success',
          data: { agentId: child.id },
          durationMs: 0,
        };
      },
      onBroadcast: async (message) => {
        this.messageBus.broadcast(runtime.agentId, message);
      },
    });

    await runtime.init();
    this.agents.set(runtime.agentId, runtime);
    this.state.agentIds.push(runtime.agentId);
    this.messageBus.registerAgent(runtime.agentId);

    return {
      id: runtime.agentId,
      run: (input: string) => runtime.run(input),
    };
  }

  async terminate(agentId: string): Promise<void> {
    const agent = this.agents.get(agentId);
    if (agent) {
      await agent.stop();
      this.agents.delete(agentId);
      this.messageBus.unregisterAgent(agentId);
    }
  }

  private async terminateAll(): Promise<void> {
    const ids = [...this.agents.keys()];
    for (const id of ids) {
      await this.terminate(id);
    }
  }

  private aggregateTokenUsage(): void {
    let prompt = 0,
      completion = 0,
      total = 0;
    for (const agent of this.agents.values()) {
      const s = agent.getState();
      prompt += s.tokenUsage.promptTokens;
      completion += s.tokenUsage.completionTokens;
      total += s.tokenUsage.totalTokens;
    }
    this.state.tokenUsage = {
      promptTokens: prompt,
      completionTokens: completion,
      totalTokens: total,
    };
  }

  private getStrategy() {
    switch (this.config.strategy) {
      case 'supervisor':
        return supervisorStrategy;
      case 'dynamic':
        return dynamicStrategy;
      case 'round-robin':
        return roundRobinStrategy;
      default:
        throw new Error(`Unknown strategy: ${this.config.strategy}`);
    }
  }

  getState(): SwarmState {
    // Only re-aggregate if agents are still alive (persistent mode)
    // In single-shot mode agents are cleared by terminateAll(), so preserve the last captured value
    if (this.agents.size > 0) {
      this.aggregateTokenUsage();
    }
    return { ...this.state };
  }

  getMessageBus(): MessageBus {
    return this.messageBus;
  }
}
