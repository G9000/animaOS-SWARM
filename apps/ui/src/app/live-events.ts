export type SectionId = 'agents' | 'swarms' | 'search' | 'health';
export type Tone = 'neutral' | 'success' | 'error';

export interface HealthSnapshot {
  status: string;
  agents: number;
  swarms: number;
  uptime: number;
}

export interface TokenUsage {
  totalTokens: number;
  promptTokens?: number;
  completionTokens?: number;
}

export interface AgentEntry {
  id: string;
  name: string;
  status: string;
  tokenUsage?: TokenUsage;
}

export interface SwarmEntry {
  id: string;
  status: string;
  agentIds?: string[];
  results?: unknown[];
  tokenUsage?: TokenUsage;
  startedAt?: number;
  completedAt?: number;
}

export interface DashboardSnapshot {
  health: HealthSnapshot;
  agents: AgentEntry[];
  swarms: SwarmEntry[];
}

export interface LiveEvent {
  type: string;
  agentId?: string;
  timestamp: number;
  data: unknown;
}

export interface LiveActivity {
  scope: SectionId | 'system';
  title: string;
  body: string;
  tone: Tone;
}

export interface LiveOutputDelta {
  agentOutput?: {
    id: string;
    body: string;
  };
  swarmOutput?: {
    id: string;
    body: string;
  };
}

function serializePayload(payload: unknown): string {
  if (typeof payload === 'string') {
    return payload;
  }

  if (payload === null || typeof payload === 'undefined') {
    return 'No payload returned.';
  }

  try {
    return JSON.stringify(payload, null, 2);
  } catch {
    return String(payload);
  }
}

function asRecord(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    return {};
  }

  return value as Record<string, unknown>;
}

function readString(
  record: Record<string, unknown>,
  key: string
): string | undefined {
  const value = record[key];
  return typeof value === 'string' ? value : undefined;
}

function readNumber(
  record: Record<string, unknown>,
  key: string
): number | undefined {
  const value = record[key];
  return typeof value === 'number' ? value : undefined;
}

function readTokenUsage(value: unknown): TokenUsage | undefined {
  const record = asRecord(value);
  const totalTokens = readNumber(record, 'totalTokens');
  if (typeof totalTokens !== 'number') {
    return undefined;
  }

  return {
    totalTokens,
    promptTokens: readNumber(record, 'promptTokens'),
    completionTokens: readNumber(record, 'completionTokens'),
  };
}

function resolveAgentId(
  event: LiveEvent,
  data: Record<string, unknown>
): string | undefined {
  return event.agentId ?? readString(data, 'agentId');
}

function resolveSwarmId(data: Record<string, unknown>): string | undefined {
  return readString(data, 'swarmId');
}

function upsertAgent(
  agents: AgentEntry[],
  nextAgent: AgentEntry
): AgentEntry[] {
  const index = agents.findIndex((agent) => agent.id === nextAgent.id);
  if (index === -1) {
    return [nextAgent, ...agents];
  }

  const current = agents[index];
  const merged = {
    ...current,
    ...nextAgent,
    tokenUsage: nextAgent.tokenUsage ?? current.tokenUsage,
  };

  return agents.map((agent, currentIndex) =>
    currentIndex === index ? merged : agent
  );
}

function upsertSwarm(
  swarms: SwarmEntry[],
  nextSwarm: SwarmEntry
): SwarmEntry[] {
  const index = swarms.findIndex((swarm) => swarm.id === nextSwarm.id);
  if (index === -1) {
    return [nextSwarm, ...swarms];
  }

  const current = swarms[index];
  const merged = {
    ...current,
    ...nextSwarm,
    agentIds: nextSwarm.agentIds ?? current.agentIds,
    results: nextSwarm.results ?? current.results,
    tokenUsage: nextSwarm.tokenUsage ?? current.tokenUsage,
    startedAt: nextSwarm.startedAt ?? current.startedAt,
    completedAt: nextSwarm.completedAt ?? current.completedAt,
  };

  return swarms.map((swarm, currentIndex) =>
    currentIndex === index ? merged : swarm
  );
}

function withUpdatedCounts(
  snapshot: DashboardSnapshot,
  agents: AgentEntry[],
  swarms: SwarmEntry[]
): DashboardSnapshot {
  return {
    health: {
      ...snapshot.health,
      agents: agents.length,
      swarms: swarms.length,
    },
    agents,
    swarms,
  };
}

function getTaskResultBody(result: unknown): string {
  const record = asRecord(result);
  const status = readString(record, 'status');
  if (status === 'error') {
    return readString(record, 'error') ?? serializePayload(result);
  }

  if ('data' in record) {
    return serializePayload(record.data);
  }

  return serializePayload(result);
}

export function applyLiveEvent(
  snapshot: DashboardSnapshot,
  event: LiveEvent
): DashboardSnapshot {
  const data = asRecord(event.data);

  switch (event.type) {
    case 'agent:spawned': {
      const agentId = resolveAgentId(event, data);
      if (!agentId) {
        return snapshot;
      }

      const existing = snapshot.agents.find((agent) => agent.id === agentId);
      return withUpdatedCounts(
        snapshot,
        upsertAgent(snapshot.agents, {
          id: agentId,
          name: readString(data, 'name') ?? existing?.name ?? agentId,
          status: existing?.status ?? 'idle',
          tokenUsage: existing?.tokenUsage,
        }),
        snapshot.swarms
      );
    }

    case 'agent:started':
    case 'task:started': {
      const agentId = resolveAgentId(event, data);
      if (!agentId) {
        return snapshot;
      }

      const existing = snapshot.agents.find((agent) => agent.id === agentId);
      return withUpdatedCounts(
        snapshot,
        upsertAgent(snapshot.agents, {
          id: agentId,
          name: existing?.name ?? agentId,
          status: 'running',
          tokenUsage: existing?.tokenUsage,
        }),
        snapshot.swarms
      );
    }

    case 'task:completed': {
      const agentId = resolveAgentId(event, data);
      if (!agentId) {
        return snapshot;
      }

      const result = asRecord(data.result);
      const existing = snapshot.agents.find((agent) => agent.id === agentId);
      return withUpdatedCounts(
        snapshot,
        upsertAgent(snapshot.agents, {
          id: agentId,
          name: existing?.name ?? agentId,
          status:
            readString(result, 'status') === 'error' ? 'failed' : 'completed',
          tokenUsage: existing?.tokenUsage,
        }),
        snapshot.swarms
      );
    }

    case 'agent:failed':
    case 'task:failed': {
      const agentId = resolveAgentId(event, data);
      if (!agentId) {
        return snapshot;
      }

      const existing = snapshot.agents.find((agent) => agent.id === agentId);
      return withUpdatedCounts(
        snapshot,
        upsertAgent(snapshot.agents, {
          id: agentId,
          name: existing?.name ?? agentId,
          status: 'failed',
          tokenUsage: existing?.tokenUsage,
        }),
        snapshot.swarms
      );
    }

    case 'agent:terminated': {
      const agentId = resolveAgentId(event, data);
      if (!agentId) {
        return snapshot;
      }

      return withUpdatedCounts(
        snapshot,
        snapshot.agents.filter((agent) => agent.id !== agentId),
        snapshot.swarms
      );
    }

    case 'agent:tokens': {
      const agentId = resolveAgentId(event, data);
      if (!agentId) {
        return snapshot;
      }

      const usage = readTokenUsage(data.usage);
      if (!usage) {
        return snapshot;
      }

      const existing = snapshot.agents.find((agent) => agent.id === agentId);
      return withUpdatedCounts(
        snapshot,
        upsertAgent(snapshot.agents, {
          id: agentId,
          name: existing?.name ?? agentId,
          status: existing?.status ?? 'idle',
          tokenUsage: usage,
        }),
        snapshot.swarms
      );
    }

    case 'swarm:created': {
      const swarmId = resolveSwarmId(data);
      if (!swarmId) {
        return snapshot;
      }

      const existing = snapshot.swarms.find((swarm) => swarm.id === swarmId);
      return withUpdatedCounts(
        snapshot,
        snapshot.agents,
        upsertSwarm(snapshot.swarms, {
          id: swarmId,
          status: existing?.status ?? 'idle',
          agentIds: existing?.agentIds,
          results: existing?.results,
          tokenUsage: existing?.tokenUsage,
          startedAt: existing?.startedAt ?? event.timestamp,
          completedAt: existing?.completedAt,
        })
      );
    }

    case 'swarm:completed': {
      const swarmId = resolveSwarmId(data);
      if (!swarmId) {
        return snapshot;
      }

      const existing = snapshot.swarms.find((swarm) => swarm.id === swarmId);
      const results =
        'result' in data
          ? [...(existing?.results ?? []), data.result]
          : existing?.results;

      return withUpdatedCounts(
        snapshot,
        snapshot.agents,
        upsertSwarm(snapshot.swarms, {
          id: swarmId,
          status: 'idle',
          agentIds: existing?.agentIds,
          results,
          tokenUsage: existing?.tokenUsage,
          startedAt: existing?.startedAt,
          completedAt: event.timestamp,
        })
      );
    }

    case 'swarm:stopped': {
      const swarmId = resolveSwarmId(data);
      if (!swarmId) {
        return snapshot;
      }

      const existing = snapshot.swarms.find((swarm) => swarm.id === swarmId);
      return withUpdatedCounts(
        snapshot,
        snapshot.agents,
        upsertSwarm(snapshot.swarms, {
          id: swarmId,
          status: 'idle',
          agentIds: existing?.agentIds,
          results: existing?.results,
          tokenUsage: existing?.tokenUsage,
          startedAt: existing?.startedAt,
          completedAt: event.timestamp,
        })
      );
    }

    default:
      return snapshot;
  }
}

export function buildLiveActivity(event: LiveEvent): LiveActivity | null {
  const data = asRecord(event.data);

  switch (event.type) {
    case 'agent:spawned': {
      const agentId = resolveAgentId(event, data);
      const name = readString(data, 'name') ?? agentId;
      if (!agentId || !name) {
        return null;
      }

      return {
        scope: 'agents',
        title: `Live agent detected: ${name}`,
        body: `Agent ${agentId} is available on the local runtime.`,
        tone: 'success',
      };
    }

    case 'task:started': {
      const agentId = resolveAgentId(event, data);
      if (!agentId) {
        return null;
      }

      return {
        scope: 'agents',
        title: 'Live agent task started',
        body: `Agent ${agentId} is executing a task.`,
        tone: 'neutral',
      };
    }

    case 'task:completed': {
      const agentId = resolveAgentId(event, data);
      if (!agentId) {
        return null;
      }

      return {
        scope: 'agents',
        title: 'Live agent task completed',
        body: getTaskResultBody(data.result),
        tone: 'success',
      };
    }

    case 'task:failed': {
      const agentId = resolveAgentId(event, data);
      if (!agentId) {
        return null;
      }

      return {
        scope: 'agents',
        title: 'Live agent task failed',
        body: readString(data, 'error') ?? 'Agent task failed.',
        tone: 'error',
      };
    }

    case 'agent:terminated': {
      const agentId = resolveAgentId(event, data);
      if (!agentId) {
        return null;
      }

      return {
        scope: 'agents',
        title: 'Live agent terminated',
        body: `Agent ${agentId} was removed from the local runtime.`,
        tone: 'neutral',
      };
    }

    case 'swarm:created': {
      const swarmId = resolveSwarmId(data);
      const strategy = readString(data, 'strategy');
      if (!swarmId || !strategy) {
        return null;
      }

      return {
        scope: 'swarms',
        title: `Live swarm staged: ${strategy}`,
        body: `Coordinator ${swarmId} is available for delegated work.`,
        tone: 'success',
      };
    }

    case 'swarm:completed': {
      const swarmId = resolveSwarmId(data);
      if (!swarmId) {
        return null;
      }

      return {
        scope: 'swarms',
        title: 'Live swarm task completed',
        body: serializePayload(data.result),
        tone: 'success',
      };
    }

    case 'swarm:stopped': {
      const swarmId = resolveSwarmId(data);
      if (!swarmId) {
        return null;
      }

      return {
        scope: 'swarms',
        title: 'Live swarm stopped',
        body: `Coordinator ${swarmId} has stopped its active pool.`,
        tone: 'neutral',
      };
    }

    default:
      return null;
  }
}

export function buildLiveOutputDelta(event: LiveEvent): LiveOutputDelta | null {
  const data = asRecord(event.data);

  switch (event.type) {
    case 'task:completed': {
      const agentId = resolveAgentId(event, data);
      if (!agentId) {
        return null;
      }

      return {
        agentOutput: {
          id: agentId,
          body: getTaskResultBody(data.result),
        },
      };
    }

    case 'task:failed': {
      const agentId = resolveAgentId(event, data);
      if (!agentId) {
        return null;
      }

      return {
        agentOutput: {
          id: agentId,
          body: readString(data, 'error') ?? 'Agent task failed.',
        },
      };
    }

    case 'swarm:completed': {
      const swarmId = resolveSwarmId(data);
      if (!swarmId) {
        return null;
      }

      return {
        swarmOutput: {
          id: swarmId,
          body: serializePayload(data.result),
        },
      };
    }

    default:
      return null;
  }
}
