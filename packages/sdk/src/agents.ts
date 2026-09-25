import type { Action, AgentConfig, Content, Plugin } from '@animaOS-SWARM/core';

import type { DaemonClient } from './client.js';
import type {
  DaemonAgentMessage,
  DaemonAgentState,
  DaemonTaskResult,
  DaemonToolDescriptor,
} from './daemon-types.js';

export interface AgentSnapshot {
  state: DaemonAgentState;
  messageCount: number;
  messages: DaemonAgentMessage[];
  eventCount: number;
  lastTask: DaemonTaskResult | null;
}

/** An agent without its transcript (`GET /api/agents?view=summary`). */
export interface AgentSummary {
  state: DaemonAgentState;
  messageCount: number;
  eventCount: number;
  lastTask: DaemonTaskResult | null;
}

export interface AgentMemory {
  id: string;
  agentId: string;
  agentName: string;
  type: string;
  content: string;
  importance: number;
  createdAt: number;
  tags?: string[] | null;
  scope: 'shared' | 'private' | 'room';
  roomId?: string | null;
  worldId?: string | null;
  sessionId?: string | null;
}

export interface AgentRunResponse {
  agent: AgentSnapshot;
  result: DaemonTaskResult;
}

export interface AgentRunOptions {
  /** Reuse this ID for subsequent turns in the same direct conversation. */
  roomId?: string;
}

export interface AgentTask {
  content: string;
  status: 'pending' | 'in_progress' | 'completed';
  activeForm: string;
}

export interface AgentTasks {
  tasks: AgentTask[];
  revision: string;
}

export interface AgentJobReviewInput {
  revision: number;
  decision: 'accepted' | 'changes_requested';
  note?: string;
}

export interface AgentJobAttempt {
  attempt: number;
  status: 'completed' | 'failed' | 'needs_review';
  startedAtMs: number;
  finishedAtMs: number;
  result: string | null;
  error: string | null;
  resultTruncated: boolean;
  review: {
    decision: 'accepted' | 'changes_requested';
    note: string;
    reviewedAtMs: number;
  } | null;
}

export interface AgentJob {
  goalId: string | null;
  id: string;
  agentId: string;
  title: string;
  prompt: string;
  requestKey: string;
  status:
    | 'awaiting_approval'
    | 'queued'
    | 'running'
    | 'completed'
    | 'failed'
    | 'needs_review'
    | 'cancelled';
  revision: number;
  attempt: number;
  maxAttempts: number;
  requiresApproval: boolean;
  approvedAtMs: number | null;
  attempts: AgentJobAttempt[];
  createdAtMs: number;
  updatedAtMs: number;
  startedAtMs: number | null;
  finishedAtMs: number | null;
  result: string | null;
  error: string | null;
}

export interface AgentJobInput {
  goalId?: string | null;
  maxAttempts?: number;
  requiresApproval?: boolean;
  title: string;
  prompt: string;
  /** Retain this key when retrying an uncertain create with identical input. */
  requestKey: string;
}

export interface AgentJobRetryInput {
  revision: number;
  acknowledgeUncertain: boolean;
}

export interface AgentScheduleInput {
  prompt: string;
  trigger:
    | { type: 'interval'; intervalMs: number }
    | { type: 'daily'; hour: number; minute: number; timeZone: string };
  target: { type: 'workspace' } | { type: 'connector'; connectorId: string };
  enabled?: boolean;
}

export interface AgentSchedule extends AgentScheduleInput {
  id: string;
  agentId: string;
  enabled: boolean;
  nextDueAtMs: number;
  lastFiredAtMs: number | null;
  lastOutcome: {
    status: 'silent' | 'spoke' | 'error';
    occurredAtMs: number;
    errorCode: string | null;
  } | null;
  createdAtMs: number;
  updatedAtMs: number;
}

export interface AgentUpdateInput {
  name?: string;
  model?: string;
  /** An empty string restores the daemon default. */
  provider?: string;
  /** An empty string clears the custom system prompt. */
  system?: string;
  tools?: AgentToolInput[];
}

/** A registered daemon tool name or a JSON descriptor. */
export type AgentToolInput =
  | string
  | {
      name: string;
      description?: string;
      parameters?: Record<string, unknown>;
      examples?: DaemonToolDescriptor['examples'];
    };

export function agent<T extends AgentConfig>(config: T): T {
  return config;
}

export function plugin<T extends Plugin>(config: T): T {
  return config;
}

export function action<T extends Action>(config: T): T {
  return config;
}

export class AgentsClient {
  constructor(private readonly client: DaemonClient) {}

  async create(config: AgentConfig): Promise<AgentSnapshot> {
    const response = await this.client.requestJson<{ agent: AgentSnapshot }>(
      '/api/agents',
      {
        method: 'POST',
        body: config,
      },
    );

    return response.agent;
  }

  async list(): Promise<AgentSnapshot[]> {
    const response = await this.client.requestJson<{ agents: AgentSnapshot[] }>(
      '/api/agents',
    );
    return response.agents;
  }

  async listSummaries(): Promise<AgentSummary[]> {
    const response = await this.client.requestJson<{ agents: AgentSummary[] }>(
      '/api/agents?view=summary',
    );
    return response.agents;
  }

  async get(agentId: string): Promise<AgentSnapshot> {
    const response = await this.client.requestJson<{ agent: AgentSnapshot }>(
      `/api/agents/${encodeURIComponent(agentId)}`,
    );
    return response.agent;
  }

  async update(
    agentId: string,
    patch: AgentUpdateInput,
  ): Promise<AgentSnapshot> {
    const response = await this.client.requestJson<{ agent: AgentSnapshot }>(
      `/api/agents/${encodeURIComponent(agentId)}`,
      { method: 'PATCH', body: patch },
    );
    return response.agent;
  }

  async remove(agentId: string): Promise<void> {
    await this.client.requestJson<{ deleted: boolean }>(
      `/api/agents/${encodeURIComponent(agentId)}`,
      { method: 'DELETE' },
    );
  }

  /** Store a PNG, JPEG, or WebP avatar (up to 5 MiB) in the workspace. */
  async setAvatar(agentId: string, image: Blob): Promise<void> {
    await this.client.requestJson(
      `/api/agents/${encodeURIComponent(agentId)}/avatar`,
      {
        method: 'PUT',
        body: image,
        headers: { 'content-type': image.type },
      },
    );
  }

  async tasks(agentId: string): Promise<AgentTasks> {
    return this.client.requestJson(
      `/api/agents/${encodeURIComponent(agentId)}/tasks`,
    );
  }

  async jobs(
    agentId: string,
    options: { signal?: AbortSignal } = {},
  ): Promise<AgentJob[]> {
    const result = await this.client.requestJson<{ jobs: AgentJob[] }>(
      `/api/agents/${encodeURIComponent(agentId)}/jobs`,
      options,
    );
    return result.jobs;
  }

  /** Explicitly queues execution; requires daemon control-plane persistence. */
  async createJob(agentId: string, input: AgentJobInput): Promise<AgentJob> {
    return this.client.requestJson(
      `/api/agents/${encodeURIComponent(agentId)}/jobs`,
      { method: 'POST', body: input },
    );
  }

  async cancelJob(
    agentId: string,
    jobId: string,
    input: { revision: number },
  ): Promise<AgentJob> {
    return this.client.requestJson(
      `/api/agents/${encodeURIComponent(agentId)}/jobs/${encodeURIComponent(jobId)}/cancel`,
      { method: 'POST', body: input },
    );
  }

  async retryJob(
    agentId: string,
    jobId: string,
    input: AgentJobRetryInput,
  ): Promise<AgentJob> {
    return this.client.requestJson(
      `/api/agents/${encodeURIComponent(agentId)}/jobs/${encodeURIComponent(jobId)}/retry`,
      { method: 'POST', body: input },
    );
  }

  async approveJob(
    agentId: string,
    jobId: string,
    input: { revision: number },
  ): Promise<AgentJob> {
    return this.client.requestJson(
      `/api/agents/${encodeURIComponent(agentId)}/jobs/${encodeURIComponent(jobId)}/approve`,
      { method: 'POST', body: input },
    );
  }

  async reviewJob(
    agentId: string,
    jobId: string,
    input: AgentJobReviewInput,
  ): Promise<AgentJob> {
    return this.client.requestJson(
      `/api/agents/${encodeURIComponent(agentId)}/jobs/${encodeURIComponent(jobId)}/review`,
      { method: 'POST', body: input },
    );
  }

  async schedules(agentId: string): Promise<AgentSchedule[]> {
    const result = await this.client.requestJson<{
      schedules: AgentSchedule[];
    }>(`/api/agents/${encodeURIComponent(agentId)}/schedules`);
    return result.schedules;
  }

  async createSchedule(
    agentId: string,
    input: AgentScheduleInput,
  ): Promise<AgentSchedule> {
    const result = await this.client.requestJson<{ schedule: AgentSchedule }>(
      `/api/agents/${encodeURIComponent(agentId)}/schedules`,
      { method: 'POST', body: input },
    );
    return result.schedule;
  }

  async updateSchedule(
    agentId: string,
    scheduleId: string,
    patch: Partial<AgentScheduleInput>,
  ): Promise<AgentSchedule> {
    const result = await this.client.requestJson<{ schedule: AgentSchedule }>(
      `/api/agents/${encodeURIComponent(agentId)}/schedules/${encodeURIComponent(scheduleId)}`,
      { method: 'PATCH', body: patch },
    );
    return result.schedule;
  }

  async removeSchedule(agentId: string, scheduleId: string): Promise<void> {
    await this.client.requestJson(
      `/api/agents/${encodeURIComponent(agentId)}/schedules/${encodeURIComponent(scheduleId)}`,
      { method: 'DELETE' },
    );
  }

  /** Pass the revision from the last read; a stale update returns HTTP 409. */
  async updateTasks(agentId: string, input: AgentTasks): Promise<AgentTasks> {
    return this.client.requestJson(
      `/api/agents/${encodeURIComponent(agentId)}/tasks`,
      { method: 'PUT', body: input },
    );
  }

  async removeAvatar(agentId: string): Promise<void> {
    await this.client.requestJson(
      `/api/agents/${encodeURIComponent(agentId)}/avatar`,
      { method: 'DELETE' },
    );
  }

  async run(
    agentId: string,
    input: Content,
    options: AgentRunOptions = {},
  ): Promise<AgentRunResponse> {
    return this.client.requestJson<AgentRunResponse>(
      `/api/agents/${encodeURIComponent(agentId)}/run`,
      {
        method: 'POST',
        body: { ...input, roomId: options.roomId },
      },
    );
  }

  /** Runs one bounded peer request and returns the recipient's snapshot and result. */
  async sendMessage(
    senderId: string,
    toAgentId: string,
    message: string,
  ): Promise<AgentRunResponse> {
    return this.client.requestJson<AgentRunResponse>(
      `/api/agents/${encodeURIComponent(senderId)}/messages`,
      { method: 'POST', body: { toAgentId, message } },
    );
  }

  async recentMemories(
    agentId: string,
    options: {
      limit?: number;
    } = {},
  ): Promise<AgentMemory[]> {
    const search = new URLSearchParams();
    if (options.limit !== undefined) {
      search.set('limit', String(options.limit));
    }

    const encodedId = encodeURIComponent(agentId);
    const path = search.size
      ? `/api/agents/${encodedId}/memories/recent?${search.toString()}`
      : `/api/agents/${encodedId}/memories/recent`;

    const response = await this.client.requestJson<{ memories: AgentMemory[] }>(
      path,
    );
    return response.memories;
  }
}
