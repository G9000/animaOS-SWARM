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

export interface SwarmState {
  id: string;
  status: string;
  agentIds: string[];
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

  run: (id: string, task: string) =>
    request<{ agent: AgentSnapshot; result: TaskResult }>(
      `/api/agents/${id}/run`,
      { method: 'POST', json: { task } }
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
