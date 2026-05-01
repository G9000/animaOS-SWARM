export interface HealthResponse {
  status: string;
  version?: string;
  uptime_secs?: number;
}

export interface TokenUsage {
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
}

export interface TaskResult {
  status: 'success' | 'error';
  data?: unknown;
  error?: string;
  durationMs?: number;
}

export interface Content {
  text: string;
  attachments?: unknown[] | null;
  metadata?: Record<string, unknown> | null;
}

export type MemoryType = 'fact' | 'observation' | 'task_result' | 'reflection';
export type MemoryScope = 'shared' | 'private' | 'room';

export interface Memory {
  id: string;
  agentId: string;
  agentName: string;
  type: MemoryType;
  content: string;
  importance: number;
  createdAt: number;
  tags?: string[] | null;
  scope: MemoryScope;
  roomId?: string | null;
  worldId?: string | null;
  sessionId?: string | null;
}

export type RelationshipEndpointKind = 'agent' | 'user' | 'system' | 'external';

export interface AgentRelationship {
  id: string;
  sourceKind: RelationshipEndpointKind;
  sourceAgentId: string;
  sourceAgentName: string;
  targetKind: RelationshipEndpointKind;
  targetAgentId: string;
  targetAgentName: string;
  relationshipType: string;
  summary?: string | null;
  strength: number;
  confidence: number;
  evidenceMemoryIds: string[];
  tags?: string[] | null;
  roomId?: string | null;
  worldId?: string | null;
  sessionId?: string | null;
  createdAt: number;
  updatedAt: number;
}

export interface AgentRelationshipOptions {
  entityId?: string;
  agentId?: string;
  sourceKind?: RelationshipEndpointKind;
  sourceAgentId?: string;
  targetKind?: RelationshipEndpointKind;
  targetAgentId?: string;
  relationshipType?: string;
  roomId?: string;
  worldId?: string;
  sessionId?: string;
  minStrength?: number;
  minConfidence?: number;
  limit?: number;
}

export interface RecentMemoriesOptions {
  agentId?: string;
  agentName?: string;
  scope?: MemoryScope;
  roomId?: string;
  worldId?: string;
  sessionId?: string;
  limit?: number;
}

export interface MemoryEntity {
  kind: RelationshipEndpointKind;
  id: string;
  name: string;
  aliases: string[];
  summary?: string | null;
  createdAt: number;
  updatedAt: number;
}

export interface MemoryEntityOptions {
  entityId?: string;
  kind?: RelationshipEndpointKind;
  name?: string;
  alias?: string;
  limit?: number;
}

export type MemoryEvaluationDecision = 'store' | 'merge' | 'ignore';

export interface MemoryEvaluation {
  decision: MemoryEvaluationDecision;
  reason: string;
  score: number;
  suggestedImportance: number;
  duplicateMemoryId?: string | null;
}

export interface EvaluatedMemoryInput {
  agentId: string;
  agentName: string;
  type: MemoryType;
  content: string;
  importance: number;
  tags?: string[] | null;
  scope?: MemoryScope;
  roomId?: string;
  worldId?: string;
  sessionId?: string;
  minContentChars?: number;
  minImportance?: number;
}

export interface MemoryEvaluationOutcome {
  evaluation: MemoryEvaluation;
  memory?: Memory | null;
}

export interface MemoryRecallOptions {
  entityId?: string;
  agentId?: string;
  agentName?: string;
  type?: MemoryType;
  scope?: MemoryScope;
  roomId?: string;
  worldId?: string;
  sessionId?: string;
  limit?: number;
  lexicalLimit?: number;
  recentLimit?: number;
  relationshipLimit?: number;
  temporalLimit?: number;
  minImportance?: number;
}

export interface MemoryRecallResult {
  memory: Memory;
  score: number;
  lexicalScore: number;
  vectorScore: number;
  relationshipScore: number;
  temporalScore: number;
  recencyScore: number;
  importanceScore: number;
}

export interface MemoryEvidenceTrace {
  memory: Memory;
  relationships: AgentRelationship[];
  entities: MemoryEntity[];
}

export interface MemoryImportanceAdjustment {
  memoryId: string;
  previousImportance: number;
  newImportance: number;
}

export interface MemoryRetentionInput {
  maxAgeMillis?: number;
  minImportance?: number;
  maxMemories?: number;
  decayHalfLifeMillis?: number;
}

export interface MemoryRetentionReport {
  decayedMemories: MemoryImportanceAdjustment[];
  removedMemoryIds: string[];
  removedRelationshipIds: string[];
}

export interface MemoryEmbeddingStatus {
  enabled: boolean;
  provider: string;
  model: string;
  dimension: number;
  vectorCount: number;
  persisted: boolean;
  storageFile?: string | null;
}

export interface MemoryEvalCheckResult {
  name: string;
  passed: boolean;
  detail: string;
}

export interface MemoryEvalCaseResult {
  name: string;
  checks: MemoryEvalCheckResult[];
}

export interface MemoryEvalReport {
  passed: boolean;
  totalChecks: number;
  passedChecks: number;
  failureMessages: string[];
  cases: MemoryEvalCaseResult[];
}

export interface MemoryReadiness {
  passed: boolean;
  embeddings: MemoryEmbeddingStatus;
  evaluation: MemoryEvalReport;
}

export interface AgentConfig {
  name: string;
  model: string;
  bio?: string;
  lore?: string;
  knowledge?: string[];
  topics?: string[];
  adjectives?: string[];
  style?: string;
  system?: string;
  provider?: string;
  tools?: string[];
  settings?: { temperature?: number; maxTokens?: number };
}

export interface AgencyCreateRequest {
  name?: string;
  description?: string;
  teamSize?: number;
  provider?: string;
  model?: string;
  modelPool?: string[];
  outputDir?: string;
  seedMemories?: boolean;
  overwrite?: boolean;
}

export interface AgentDefinitionResponse {
  name: string;
  position?: string;
  role: 'orchestrator' | 'worker';
  bio?: string;
  lore?: string;
  adjectives?: string[];
  topics?: string[];
  knowledge?: string[];
  style?: string;
  system?: string;
  model?: string;
  tools?: string[];
  collaboratesWith?: string[];
}

export interface AgencyGenerateResponse {
  name: string;
  description: string;
  provider: string;
  model: string;
  teamSize: number;
  mission?: string;
  values?: string[];
  agents: AgentDefinitionResponse[];
}

export interface AgencyCreateResponse {
  agency: AgencyGenerateResponse;
  outputDir: string;
  files: string[];
  seedMemoryCount: number;
  seededAgents: number;
}

export interface AgentState {
  id: string;
  name: string;
  status: string;
  config?: AgentConfig;
  createdAt: number;
  tokenUsage: TokenUsage;
}

export interface AgentSnapshot {
  state: AgentState;
  messageCount: number;
  eventCount: number;
  lastTask?: TaskResult;
}

export interface AgentMessage {
  id: string;
  from: string;
  to: string;
  content: Content;
  timestamp: number;
}

export interface SwarmState {
  id: string;
  status: string;
  agentIds: string[];
  messages: AgentMessage[];
  results: TaskResult[];
  tokenUsage: TokenUsage;
  startedAt?: number;
  completedAt?: number;
}

export interface SwarmCreateRequest {
  strategy: 'supervisor' | 'dynamic' | 'round-robin';
  manager: AgentConfig;
  workers: AgentConfig[];
  maxConcurrentAgents?: number;
  maxParallelDelegations?: number;
  maxTurns?: number;
  tokenBudget?: number;
}

async function request<T>(
  path: string,
  init?: RequestInit & { json?: unknown }
): Promise<T> {
  const headers = new Headers(init?.headers);
  let body: BodyInit | undefined;

  if (init?.json !== undefined) {
    headers.set('content-type', 'application/json');
    body = JSON.stringify(init.json);
  }

  const res = await fetch(path, { ...init, headers, body });
  const text = await res.text();
  const data = text ? (JSON.parse(text) as unknown) : undefined;

  if (!res.ok) {
    const msg =
      data && typeof data === 'object' && 'error' in data
        ? String((data as { error: unknown }).error)
        : `HTTP ${res.status}`;
    throw new Error(msg);
  }

  return data as T;
}

export const health = {
  get: () => request<HealthResponse>('/api/health'),
};

export interface Provider {
  id: string;
  label: string;
  requiresKey: boolean;
  configured: boolean;
  apiKeyEnvs: string[];
}

export const providers = {
  list: () =>
    request<{ providers: Provider[] }>('/api/providers').then(
      (r) => r.providers
    ),
};

export const agencies = {
  create: (body: AgencyCreateRequest) =>
    request<AgencyCreateResponse>('/api/agencies/create', {
      method: 'POST',
      json: body,
    }),
};

export const agents = {
  list: () =>
    request<{ agents: AgentSnapshot[] }>('/api/agents').then((r) => r.agents),

  get: (id: string) =>
    request<{ agent: AgentSnapshot }>(`/api/agents/${id}`).then((r) => r.agent),

  create: (config: AgentConfig) =>
    request<{ agent: AgentSnapshot }>('/api/agents', {
      method: 'POST',
      json: config,
    }).then((r) => r.agent),

  run: (id: string, task: string, metadata?: Record<string, string>) =>
    request<{ agent: AgentSnapshot; result: TaskResult }>(
      `/api/agents/${id}/run`,
      { method: 'POST', json: metadata ? { task, metadata } : { task } }
    ),

  delete: (id: string) =>
    request<{ deleted: boolean }>(`/api/agents/${id}`, { method: 'DELETE' }),
};

export const swarms = {
  list: () =>
    request<{ swarms: SwarmState[] }>('/api/swarms').then((r) => r.swarms),

  get: (id: string) =>
    request<{ swarm: SwarmState }>(`/api/swarms/${id}`).then((r) => r.swarm),

  create: (config: SwarmCreateRequest) =>
    request<{ swarm: SwarmState }>('/api/swarms', {
      method: 'POST',
      json: config,
    }).then((r) => r.swarm),

  run: (id: string, task: string) =>
    request<{ swarm: SwarmState; result: TaskResult }>(
      `/api/swarms/${id}/run`,
      { method: 'POST', json: { task } }
    ),
};

export const memories = {
  recent: (opts?: RecentMemoriesOptions) => {
    const params = new URLSearchParams();
    if (opts?.agentId) params.set('agentId', opts.agentId);
    if (opts?.agentName) params.set('agentName', opts.agentName);
    if (opts?.scope) params.set('scope', opts.scope);
    if (opts?.roomId) params.set('roomId', opts.roomId);
    if (opts?.worldId) params.set('worldId', opts.worldId);
    if (opts?.sessionId) params.set('sessionId', opts.sessionId);
    if (opts?.limit !== undefined) params.set('limit', String(opts.limit));
    const qs = params.toString();
    return request<{ memories: Memory[] }>(
      `/api/memories/recent${qs ? `?${qs}` : ''}`
    ).then((r) => r.memories);
  },

  entities: (opts?: MemoryEntityOptions) => {
    const params = new URLSearchParams();
    if (opts?.entityId) params.set('entityId', opts.entityId);
    if (opts?.kind) params.set('kind', opts.kind);
    if (opts?.name) params.set('name', opts.name);
    if (opts?.alias) params.set('alias', opts.alias);
    if (opts?.limit !== undefined) params.set('limit', String(opts.limit));
    const qs = params.toString();
    return request<{ entities: MemoryEntity[] }>(
      `/api/memories/entities${qs ? `?${qs}` : ''}`
    ).then((r) => r.entities);
  },

  evaluate: (input: EvaluatedMemoryInput) =>
    request<MemoryEvaluation>('/api/memories/evaluations', {
      method: 'POST',
      json: input,
    }),

  addEvaluated: (input: EvaluatedMemoryInput) =>
    request<MemoryEvaluationOutcome>('/api/memories/evaluated', {
      method: 'POST',
      json: input,
    }),

  recall: (query: string, opts?: MemoryRecallOptions) => {
    const params = new URLSearchParams();
    params.set('q', query);
    if (opts?.entityId) params.set('entityId', opts.entityId);
    if (opts?.agentId) params.set('agentId', opts.agentId);
    if (opts?.agentName) params.set('agentName', opts.agentName);
    if (opts?.type) params.set('type', opts.type);
    if (opts?.scope) params.set('scope', opts.scope);
    if (opts?.roomId) params.set('roomId', opts.roomId);
    if (opts?.worldId) params.set('worldId', opts.worldId);
    if (opts?.sessionId) params.set('sessionId', opts.sessionId);
    if (opts?.limit !== undefined) params.set('limit', String(opts.limit));
    if (opts?.lexicalLimit !== undefined) params.set('lexicalLimit', String(opts.lexicalLimit));
    if (opts?.recentLimit !== undefined) params.set('recentLimit', String(opts.recentLimit));
    if (opts?.relationshipLimit !== undefined) params.set('relationshipLimit', String(opts.relationshipLimit));
    if (opts?.temporalLimit !== undefined) params.set('temporalLimit', String(opts.temporalLimit));
    if (opts?.minImportance !== undefined) params.set('minImportance', String(opts.minImportance));
    return request<{ results: MemoryRecallResult[] }>(
      `/api/memories/recall?${params.toString()}`
    ).then((r) => r.results);
  },

  trace: (memoryId: string) =>
    request<MemoryEvidenceTrace>(
      `/api/memories/${encodeURIComponent(memoryId)}/trace`
    ),

  applyRetention: (input: MemoryRetentionInput) =>
    request<MemoryRetentionReport>('/api/memories/retention', {
      method: 'POST',
      json: input,
    }),

  readiness: () => request<MemoryReadiness>('/api/memories/readiness'),

  relationships: (opts?: AgentRelationshipOptions) => {
    const params = new URLSearchParams();
    if (opts?.entityId) params.set('entityId', opts.entityId);
    if (opts?.agentId) params.set('agentId', opts.agentId);
    if (opts?.sourceKind) params.set('sourceKind', opts.sourceKind);
    if (opts?.sourceAgentId) params.set('sourceAgentId', opts.sourceAgentId);
    if (opts?.targetKind) params.set('targetKind', opts.targetKind);
    if (opts?.targetAgentId) params.set('targetAgentId', opts.targetAgentId);
    if (opts?.relationshipType) params.set('relationshipType', opts.relationshipType);
    if (opts?.roomId) params.set('roomId', opts.roomId);
    if (opts?.worldId) params.set('worldId', opts.worldId);
    if (opts?.sessionId) params.set('sessionId', opts.sessionId);
    if (opts?.minStrength !== undefined) params.set('minStrength', String(opts.minStrength));
    if (opts?.minConfidence !== undefined) params.set('minConfidence', String(opts.minConfidence));
    if (opts?.limit !== undefined) params.set('limit', String(opts.limit));
    const qs = params.toString();
    return request<{ relationships: AgentRelationship[] }>(
      `/api/memories/relationships${qs ? `?${qs}` : ''}`
    ).then((r) => r.relationships);
  },
};

export function coerceText(data: unknown): string {
  if (data == null) return '';
  if (typeof data === 'string') return data;
  if (typeof data === 'object') {
    const o = data as Record<string, unknown>;
    if (typeof o.text === 'string') return o.text;
    if (typeof o.content === 'string') return o.content;
    if (
      o.content &&
      typeof o.content === 'object' &&
      typeof (o.content as Record<string, unknown>).text === 'string'
    ) {
      return (o.content as { text: string }).text;
    }
  }
  return JSON.stringify(data, null, 2);
}
