// ── Types ────────────────────────────────────────────────────────────────────

export interface HealthResponse {
  status: string;
  version?: string;
  uptime_secs?: number;
}

export interface ProviderResponse {
  id: string;
  label: string;
  requiresKey: boolean;
  configured: boolean;
  apiKeyEnvs: string[];
}

export interface AgencyGenerateRequest {
  name?: string;
  description?: string;
  teamSize?: number;
  provider?: string;
  model?: string;
  modelPool?: string[];
}

export interface AgencyCreateRequest extends AgencyGenerateRequest {
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

export interface AgentState {
  id: string;
  name: string;
  status: string;
  createdAt: number;
  tokenUsage: TokenUsage;
}

export interface AgentSnapshot {
  state: AgentState;
  messageCount: number;
  eventCount: number;
  lastTask?: TaskResult;
}

export interface SwarmState {
  id: string;
  status: string;
  agentIds: string[];
  results: TaskResult[];
  tokenUsage: TokenUsage;
  startedAt?: number;
  completedAt?: number;
}

export interface Memory {
  id: string;
  agentId: string;
  agentName: string;
  type: string;
  content: string;
  importance: number;
  createdAt: number;
  tags?: string[];
  scope: 'shared' | 'private' | 'room';
  roomId?: string | null;
  worldId?: string | null;
  sessionId?: string | null;
}

export interface MemorySearchResult extends Memory {
  score: number;
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
}

export interface WorkerConfig extends AgentConfig {}

export interface SwarmCreateRequest {
  strategy: 'supervisor' | 'dynamic' | 'round-robin';
  manager: AgentConfig;
  workers: WorkerConfig[];
  maxConcurrentAgents?: number;
  maxParallelDelegations?: number;
  maxTurns?: number;
  tokenBudget?: number;
}

export interface MemoryCreateRequest {
  agentId: string;
  agentName: string;
  type: 'fact' | 'observation' | 'task_result' | 'reflection';
  content: string;
  importance: number;
  tags?: string[];
  scope?: 'shared' | 'private' | 'room';
  roomId?: string;
  worldId?: string;
  sessionId?: string;
}

export interface MemorySearchOptions {
  q: string;
  type?: string;
  agentId?: string;
  agentName?: string;
  scope?: 'shared' | 'private' | 'room';
  roomId?: string;
  worldId?: string;
  sessionId?: string;
  limit?: number;
  minImportance?: number;
}

// ── Core fetch helper ────────────────────────────────────────────────────────

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
        : `Request failed: ${res.status}`;
    throw new Error(msg);
  }

  return data as T;
}

// ── Health ───────────────────────────────────────────────────────────────────

export const health = {
  get: () => request<HealthResponse>('/api/health'),
};

export const providers = {
  list: () =>
    request<{ providers: ProviderResponse[] }>('/api/providers').then(
      (r) => r.providers
    ),
};

export const agencies = {
  generate: (body: AgencyGenerateRequest) =>
    request<AgencyGenerateResponse>('/api/agencies/generate', {
      method: 'POST',
      json: body,
    }),

  create: (body: AgencyCreateRequest) =>
    request<AgencyCreateResponse>('/api/agencies/create', {
      method: 'POST',
      json: body,
    }),
};

// ── Agents ───────────────────────────────────────────────────────────────────

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

  run: (id: string, task: string) =>
    request<{ agent: AgentSnapshot; result: TaskResult }>(
      `/api/agents/${id}/run`,
      { method: 'POST', json: { task } }
    ),

  delete: (id: string) =>
    request<{ deleted: boolean }>(`/api/agents/${id}`, { method: 'DELETE' }),

  recentMemories: (id: string, limit = 10) =>
    request<{ memories: Memory[] }>(
      `/api/agents/${id}/memories/recent?limit=${limit}`
    ).then((r) => r.memories),
};

// ── Swarms ───────────────────────────────────────────────────────────────────

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

  streamEvents: (
    id: string,
    onEvent: (event: { swarmId: string; state: SwarmState; result?: TaskResult }) => void,
    onClose?: () => void
  ): (() => void) => {
    const source = new EventSource(`/api/swarms/${id}/events`);
    source.onmessage = (e) => {
      try { onEvent(JSON.parse(e.data as string)); } catch { /* ignore */ }
    };
    source.onerror = () => { source.close(); onClose?.(); };
    return () => source.close();
  },
};

// ── Memories ─────────────────────────────────────────────────────────────────

export const memories = {
  create: (body: MemoryCreateRequest) =>
    request<{ memory: Memory }>('/api/memories', { method: 'POST', json: body }).then((r) => r.memory),

  search: (opts: MemorySearchOptions) => {
    const params = new URLSearchParams({ q: opts.q });
    if (opts.type) params.set('type', opts.type);
    if (opts.agentId) params.set('agentId', opts.agentId);
    if (opts.agentName) params.set('agentName', opts.agentName);
    if (opts.scope) params.set('scope', opts.scope);
    if (opts.roomId) params.set('roomId', opts.roomId);
    if (opts.worldId) params.set('worldId', opts.worldId);
    if (opts.sessionId) params.set('sessionId', opts.sessionId);
    if (opts.limit !== undefined) params.set('limit', String(opts.limit));
    if (opts.minImportance !== undefined) params.set('minImportance', String(opts.minImportance));
    return request<{ results: MemorySearchResult[] }>(
      `/api/memories/search?${params.toString()}`
    ).then((r) => r.results);
  },

  recent: (opts?: { agentId?: string; agentName?: string; scope?: 'shared' | 'private' | 'room'; roomId?: string; worldId?: string; sessionId?: string; limit?: number }) => {
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
};
