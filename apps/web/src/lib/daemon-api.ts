// Typed client for the anima-daemon server (hosts/rust-daemon).
// The daemon owns a collection of durable agent snapshots. UI surfaces choose
// the agent they need instead of relying on GET /api/agents response order.
// All URLs are relative so the Vite dev proxy owns the origin
// (see vite.config.mts: '/api' -> UI_BACKEND_ORIGIN ?? http://localhost:8080).

import { PROVIDER_MODELS, type AgentDetail, type ChatMessage } from './types';

export interface DaemonProvider {
  id: string;
  label: string;
  requiresKey: boolean;
  /** true when the daemon has a usable API key / local runtime for this provider */
  configured: boolean;
  apiKeyEnvs: string[];
}

interface DaemonTokenUsage {
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
}

interface DaemonMessage {
  id: string;
  agentId: string;
  roomId: string;
  content: { text: string; metadata?: Record<string, unknown> | null };
  role: string;
  createdAtMs: number;
}

export interface DaemonToolExample {
  input: string;
  args: Record<string, unknown>;
  output: string;
}

export interface DaemonToolDescriptor {
  name: string;
  description: string;
  parameters: Record<string, unknown>;
  examples?: DaemonToolExample[] | null;
}

export interface DaemonSnapshot {
  state: {
    id: string;
    name: string;
    status: string;
    config: {
      name: string;
      model: string;
      provider?: string | null;
      system?: string | null;
      bio?: string | null;
      adjectives?: string[] | null;
      style?: string | null;
      tools?: DaemonToolDescriptor[] | null;
    };
    createdAtMs: number;
    tokenUsage: DaemonTokenUsage;
  };
  messageCount: number;
  messages: DaemonMessage[];
  eventCount: number;
}

export interface DaemonRunResult {
  status: 'success' | 'error';
  durationMs: number;
  error?: string | null;
  data?: { text: string } | null;
}

export interface AgentUpdateInput {
  name?: string;
  model?: string;
  provider?: string;
  system?: string;
  tools?: string[];
}

export type TelegramConnectorStatus =
  | 'ready'
  | 'pairing'
  | 'credentialRequired'
  | 'error'
  | 'degraded'
  | 'reconciling';

export interface TelegramChat {
  id: string;
  kind: 'private' | 'group' | 'supergroup' | 'channel';
  title: string | null;
  username: string | null;
}

export interface TelegramConnector {
  id: string;
  agentId: string;
  roomId: string;
  type: 'telegram';
  bot: { id: string; username: string | null; displayName: string | null };
  approvedChat: TelegramChat | null;
  pendingPairing: { chat: TelegramChat; requestedAtMs: number } | null;
  status: TelegramConnectorStatus;
  enabled: boolean;
  createdAtMs: number;
  updatedAtMs: number;
}

export interface ConnectorMessage {
  id: string;
  agentId: string;
  roomId: string;
  content: { text: string; metadata?: Record<string, unknown> | null };
  role: 'user' | 'assistant' | 'system' | 'tool';
  createdAtMs: number;
}

export type ScheduleTrigger =
  | { type: 'interval'; intervalMs: number }
  | { type: 'daily'; hour: number; minute: number; timeZone: string };
export type ScheduleTarget =
  | { type: 'workspace' }
  | { type: 'connector'; connectorId: string };
export interface ScheduleOutcome {
  status: 'silent' | 'spoke' | 'error';
  occurredAtMs: number;
  errorCode: string | null;
}
export interface DaemonSchedule {
  id: string;
  importIdempotencyKey: string | null;
  agentId: string;
  prompt: string;
  trigger: ScheduleTrigger;
  enabled: boolean;
  target: ScheduleTarget;
  nextDueAtMs: number;
  lastFiredAtMs: number | null;
  lastOutcome: ScheduleOutcome | null;
  createdAtMs: number;
  updatedAtMs: number;
}

export interface ScheduleCreateInput {
  prompt: string;
  trigger: ScheduleTrigger;
  target: ScheduleTarget;
  enabled?: boolean;
  importIdempotencyKey?: string;
}

export interface LegacyScheduleInput {
  id: string;
  prompt: string;
  intervalSecs: number;
  createdAtMs: number;
  lastRunAtMs?: number;
  target?: ScheduleTarget;
}

/** Model suggestions keyed by daemon provider id. */
export const MODEL_SUGGESTIONS: Record<string, string[]> = {
  ...PROVIDER_MODELS,
  deterministic: ['deterministic'],
};

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`/api${path}`, {
    ...init,
    headers: {
      ...(init?.body ? { 'content-type': 'application/json' } : {}),
      ...init?.headers,
    },
  });
  if (!response.ok) {
    let message = `daemon request failed (${response.status})`;
    try {
      const body = (await response.json()) as {
        error?: string;
        message?: string;
      };
      message = body.error ?? body.message ?? message;
    } catch {
      /* keep default message */
    }
    const error = new Error(message) as Error & { status?: number };
    error.status = response.status;
    throw error;
  }
  if (response.status === 204) return undefined as T;
  return (await response.json()) as T;
}

/** Error.message prefix returned by the daemon when no generative provider is available. */
export const PROFILE_GENERATION_UNAVAILABLE = 'PROFILE_GENERATION_UNAVAILABLE';

export interface WorkspaceConfigInput {
  rootPath: string;
  companyName: string;
  mission: string;
  values: string[];
}

export interface DaemonWorkspaceConfig extends WorkspaceConfigInput {
  hasAvatar: boolean;
}

export interface DaemonWorkspaceState {
  configured: boolean;
  workspace: DaemonWorkspaceConfig | null;
  defaultRoot: string;
  /** Present only on validate-only responses: does the folder already exist? */
  rootPathExists?: boolean;
}

/** Validate-only response: the daemon always sets rootPathExists here, never elsewhere. */
export type DaemonWorkspaceValidation = DaemonWorkspaceState & {
  rootPathExists: boolean;
};

export const workspaceAvatarUrl = (revision: number) =>
  `/api/workspace/avatar?v=${revision}`;

export interface GenerateProfileInput {
  presetId: string;
  intent: string;
  provider: string;
  model: string;
  workspace: { companyName: string; mission: string; values: string[] };
}

export interface AgentProfile {
  bio: string;
  adjectives: string[];
  style: string;
  system: string;
}

export interface BootstrapWorkspaceInput {
  workspace: WorkspaceConfigInput;
  agent: {
    name: string;
    presetId: string;
    bio: string;
    adjectives?: string[];
    style?: string;
    system: string;
    provider?: string;
    model: string;
    tools: string[];
  };
}

export interface WorkspaceInspectAgentPreview {
  name: string;
  bio?: string;
  provider: string;
  model: string;
}

export interface WorkspaceInspectFound {
  found: true;
  companyName: string;
  /** Omitted when the yaml lacks a mission (description fallback may be blank). */
  mission?: string;
  /** Omitted when the yaml lacks values. */
  values?: string[];
  orchestrator: WorkspaceInspectAgentPreview;
  workers: WorkspaceInspectAgentPreview[];
  providerAvailable: boolean;
}

export type WorkspaceInspectResponse = { found: false } | WorkspaceInspectFound;

export interface WorkspaceResumeResponse {
  workspace: DaemonWorkspaceConfig;
  orchestrator: DaemonSnapshot;
  workers: DaemonSnapshot[];
  skipped: string[];
}

export const daemon = {
  health: () => request<{ status: string }>('/health'),

  listProviders: () => request<{ providers: DaemonProvider[] }>('/providers'),

  listAgents: () => request<{ agents: DaemonSnapshot[] }>('/agents'),

  getAgent: (id: string) => request<{ agent: DaemonSnapshot }>(`/agents/${id}`),

  /** name, model, and tool names are required by the daemon. */
  createAgent: (input: {
    name: string;
    model: string;
    tools: string[];
    provider?: string;
    system?: string;
  }) =>
    request<{ agent: DaemonSnapshot }>('/agents', {
      method: 'POST',
      body: JSON.stringify(input),
    }),

  deleteAgent: (id: string) =>
    request<{ deleted: boolean }>(`/agents/${id}`, { method: 'DELETE' }),

  /**
   * Partial config update; the conversation is kept. Empty string for
   * provider/system clears the field back to the daemon default.
   */
  updateAgent: (id: string, patch: AgentUpdateInput) =>
    request<{ agent: DaemonSnapshot }>(`/agents/${id}`, {
      method: 'PATCH',
      body: JSON.stringify(patch),
    }),

  /** Run one agent chat turn: user text in, task result out. */
  runAgent: (id: string, text: string, metadata?: Record<string, unknown>) =>
    request<{ agent: DaemonSnapshot; result: DaemonRunResult }>(
      `/agents/${id}/run`,
      {
        method: 'POST',
        body: JSON.stringify({ text, ...(metadata ? { metadata } : {}) }),
      },
    ),

  listConnectors: (agentId: string) =>
    request<{ connectors: TelegramConnector[] }>(
      `/agents/${encodeURIComponent(agentId)}/connectors`,
    ),
  createTelegramConnector: (agentId: string, botToken: string) =>
    request<{ connector: TelegramConnector }>(
      `/agents/${encodeURIComponent(agentId)}/connectors/telegram`,
      { method: 'POST', body: JSON.stringify({ botToken }) },
    ),
  replaceTelegramCredential: (
    agentId: string,
    connectorId: string,
    botToken: string,
  ) =>
    request<{ connector: TelegramConnector }>(
      `/agents/${encodeURIComponent(agentId)}/connectors/${encodeURIComponent(connectorId)}/credential`,
      { method: 'PUT', body: JSON.stringify({ botToken }) },
    ),
  approveTelegramPairing: (
    agentId: string,
    connectorId: string,
    chatId: string,
  ) =>
    request<{ connector: TelegramConnector }>(
      `/agents/${encodeURIComponent(agentId)}/connectors/${encodeURIComponent(connectorId)}/pairings/${encodeURIComponent(chatId)}/approve`,
      { method: 'POST' },
    ),
  restartTelegramConnector: (agentId: string, connectorId: string) =>
    request<{ connector: TelegramConnector }>(
      `/agents/${encodeURIComponent(agentId)}/connectors/${encodeURIComponent(connectorId)}/restart`,
      { method: 'POST' },
    ),
  deleteTelegramConnector: (agentId: string, connectorId: string) =>
    request<{ deleted: boolean }>(
      `/agents/${encodeURIComponent(agentId)}/connectors/${encodeURIComponent(connectorId)}`,
      { method: 'DELETE' },
    ),
  listConnectorMessages: (
    agentId: string,
    connectorId: string,
    page: { before?: string; limit?: number } = {},
  ) => {
    const query = new URLSearchParams();
    if (page.before) query.set('before', page.before);
    if (page.limit) query.set('limit', String(page.limit));
    const suffix = query.size ? `?${query}` : '';
    return request<{ messages: ConnectorMessage[]; nextBefore: string | null }>(
      `/agents/${encodeURIComponent(agentId)}/connectors/${encodeURIComponent(connectorId)}/messages${suffix}`,
    );
  },
  sendConnectorMessage: (
    agentId: string,
    connectorId: string,
    text: string,
    idempotencyKey: string,
  ) =>
    request<{
      messages: ConnectorMessage[];
      result: DaemonRunResult;
      deliveryQueued: boolean;
    }>(
      `/agents/${encodeURIComponent(agentId)}/connectors/${encodeURIComponent(connectorId)}/messages`,
      {
        method: 'POST',
        headers: { 'Idempotency-Key': idempotencyKey },
        body: JSON.stringify({ text }),
      },
    ),
  listSchedules: (agentId: string) =>
    request<{ schedules: DaemonSchedule[] }>(
      `/agents/${encodeURIComponent(agentId)}/schedules`,
    ),
  createSchedule: (agentId: string, input: ScheduleCreateInput) =>
    request<{ schedule: DaemonSchedule }>(
      `/agents/${encodeURIComponent(agentId)}/schedules`,
      { method: 'POST', body: JSON.stringify(input) },
    ),
  updateSchedule: (
    agentId: string,
    scheduleId: string,
    patch: Partial<
      Pick<DaemonSchedule, 'prompt' | 'trigger' | 'target' | 'enabled'>
    >,
  ) =>
    request<{ schedule: DaemonSchedule }>(
      `/agents/${encodeURIComponent(agentId)}/schedules/${encodeURIComponent(scheduleId)}`,
      { method: 'PATCH', body: JSON.stringify(patch) },
    ),
  deleteSchedule: (agentId: string, scheduleId: string) =>
    request<{ deleted: boolean }>(
      `/agents/${encodeURIComponent(agentId)}/schedules/${encodeURIComponent(scheduleId)}`,
      { method: 'DELETE' },
    ),
  importLegacySchedules: (
    agentId: string,
    input: { schedules: LegacyScheduleInput[] },
  ) =>
    request<{ schedules: DaemonSchedule[] }>(
      `/agents/${encodeURIComponent(agentId)}/schedules/import`,
      { method: 'POST', body: JSON.stringify(input) },
    ),

  getWorkspace: () => request<DaemonWorkspaceState>('/workspace'),

  uploadWorkspaceAvatar: (file: File) =>
    request<void>('/workspace/avatar', {
      method: 'PUT',
      headers: { 'content-type': file.type },
      body: file,
    }),

  putWorkspace: (input: WorkspaceConfigInput) =>
    request<DaemonWorkspaceState>('/workspace', {
      method: 'PUT',
      body: JSON.stringify(input),
    }),

  validateWorkspace: (input: WorkspaceConfigInput) =>
    request<DaemonWorkspaceValidation>('/workspace', {
      method: 'PUT',
      body: JSON.stringify({ ...input, validateOnly: true }),
    }),

  generateProfile: (input: GenerateProfileInput) =>
    request<{ profile: AgentProfile }>('/agents/generate-profile', {
      method: 'POST',
      body: JSON.stringify(input),
    }),

  bootstrapWorkspace: (input: BootstrapWorkspaceInput) =>
    request<{ workspace: DaemonWorkspaceConfig; agent: DaemonSnapshot }>(
      '/workspace/bootstrap',
      { method: 'POST', body: JSON.stringify(input) },
    ),

  inspectWorkspace: (rootPath: string) =>
    request<WorkspaceInspectResponse>(
      `/workspace/inspect?rootPath=${encodeURIComponent(rootPath)}`,
    ),

  resumeWorkspace: (rootPath: string) =>
    request<WorkspaceResumeResponse>('/workspace/resume', {
      method: 'POST',
      body: JSON.stringify({ rootPath }),
    }),
};

/* ── adapters: daemon wire format → UI view model ── */

function mapRole(role: string): ChatMessage['role'] {
  switch (role) {
    case 'user':
      return 'User';
    case 'assistant':
      return 'Assistant';
    default:
      return 'System';
  }
}

function mapStatus(status: string): AgentDetail['status'] {
  switch (status) {
    case 'running':
      return 'Running';
    case 'completed':
      return 'Completed';
    case 'failed':
      return 'Failed';
    case 'terminated':
      return 'Terminated';
    default:
      return 'Idle';
  }
}

export function toAgentDetail(snapshot: DaemonSnapshot): AgentDetail {
  const { state } = snapshot;
  return {
    id: state.id,
    name: state.name,
    provider: state.config.provider ?? 'default',
    model: state.config.model,
    toolNames: state.config.tools?.map((tool) => tool.name) ?? [],
    created_at_ms: state.createdAtMs,
    status: mapStatus(state.status),
    token_usage: {
      prompt_tokens: state.tokenUsage.promptTokens,
      completion_tokens: state.tokenUsage.completionTokens,
      total_tokens: state.tokenUsage.totalTokens,
    },
    system: state.config.system,
    messages: snapshot.messages
      .filter((m) => {
        // hide check-in plumbing: tagged prompt messages + silent ticks
        if (m.role === 'user' && m.content.metadata?.kind === 'checkin')
          return false;
        if (m.role === 'assistant' && m.content.text.trim() === 'CHECKIN_OK')
          return false;
        return true;
      })
      .map((m) => ({
        id: m.id,
        role: mapRole(m.role),
        content: { text: m.content.text, metadata: m.content.metadata },
        created_at_ms: m.createdAtMs,
      })),
  };
}
