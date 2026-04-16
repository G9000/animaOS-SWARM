import type { AgentEntry, SwarmEntry, TokenUsage } from './live-events';

export interface TaskOutcome {
  status: string;
  data?: unknown;
  error?: string;
  durationMs?: number;
}

export interface AgentCreateSummary {
  id: string;
  name: string;
  status?: string;
}

export interface SwarmCreateSummary {
  id: string;
  strategy?: string;
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

function readAgentEntry(value: unknown): AgentEntry | null {
  const record = asRecord(value);
  const flatId = readString(record, 'id');
  if (flatId) {
    return {
      id: flatId,
      name: readString(record, 'name') ?? flatId,
      status: readString(record, 'status') ?? 'idle',
      tokenUsage: readTokenUsage(record.tokenUsage),
    };
  }

  const state = asRecord(record.state);
  const nestedId = readString(state, 'id');
  if (!nestedId) {
    return null;
  }

  return {
    id: nestedId,
    name: readString(state, 'name') ?? nestedId,
    status: readString(state, 'status') ?? 'idle',
    tokenUsage: readTokenUsage(state.tokenUsage),
  };
}

function readSwarmEntry(value: unknown): SwarmEntry | null {
  const record = asRecord(value);
  const id = readString(record, 'id');
  if (!id) {
    return null;
  }

  return {
    id,
    status: readString(record, 'status') ?? 'idle',
    agentIds: Array.isArray(record.agentIds)
      ? record.agentIds.filter((value): value is string => typeof value === 'string')
      : undefined,
    results: Array.isArray(record.results) ? record.results : undefined,
    tokenUsage: readTokenUsage(record.tokenUsage),
    startedAt: readNumber(record, 'startedAt'),
    completedAt: readNumber(record, 'completedAt'),
  };
}

function readTaskOutcome(value: unknown): TaskOutcome | null {
  const record = asRecord(value);
  const status = readString(record, 'status');
  if (!status) {
    return null;
  }

  return {
    status,
    data: record.data,
    error: readString(record, 'error'),
    durationMs: readNumber(record, 'durationMs'),
  };
}

export function normalizeAgentListResponse(value: unknown): AgentEntry[] {
  const payload = asRecord(value);
  const entries = Array.isArray(payload.agents) ? payload.agents : [];
  return entries
    .map((entry) => readAgentEntry(entry))
    .filter((entry): entry is AgentEntry => entry !== null);
}

export function normalizeSwarmListResponse(value: unknown): SwarmEntry[] {
  const payload = asRecord(value);
  const entries = Array.isArray(payload.swarms) ? payload.swarms : [];
  return entries
    .map((entry) => readSwarmEntry(entry))
    .filter((entry): entry is SwarmEntry => entry !== null);
}

export function normalizeAgentCreateResponse(value: unknown): AgentCreateSummary {
  const record = asRecord(value);
  const flatId = readString(record, 'id');
  if (flatId) {
    return {
      id: flatId,
      name: readString(record, 'name') ?? flatId,
      status: readString(record, 'status'),
    };
  }

  const agent = readAgentEntry(record.agent);
  if (!agent) {
    throw new Error('Agent create response is invalid.');
  }

  return {
    id: agent.id,
    name: agent.name,
    status: agent.status,
  };
}

export function normalizeSwarmCreateResponse(value: unknown): SwarmCreateSummary {
  const record = asRecord(value);
  const flatId = readString(record, 'id');
  if (flatId) {
    return {
      id: flatId,
      strategy: readString(record, 'strategy'),
    };
  }

  const swarm = readSwarmEntry(record.swarm);
  if (!swarm) {
    throw new Error('Swarm create response is invalid.');
  }

  return {
    id: swarm.id,
    strategy: readString(record, 'strategy'),
  };
}

export function normalizeAgentRunResponse(value: unknown): TaskOutcome {
  const flat = readTaskOutcome(value);
  if (flat) {
    return flat;
  }

  const record = asRecord(value);
  const nested = readTaskOutcome(record.result);
  if (!nested) {
    throw new Error('Agent run response is invalid.');
  }

  return nested;
}

export function normalizeSwarmRunResponse(value: unknown): TaskOutcome {
  const flat = readTaskOutcome(value);
  if (flat) {
    return flat;
  }

  const record = asRecord(value);
  const nested = readTaskOutcome(record.result);
  if (!nested) {
    throw new Error('Swarm run response is invalid.');
  }

  return nested;
}
