import type { DaemonClient } from './client.js';
import type { AgentJob } from './agents.js';
export type GoalStatus = 'active' | 'paused' | 'completed';
export interface GoalInput {
  title: string;
  objective: string;
  requestKey: string;
  maxAttempts: number;
}
export interface GoalView extends GoalInput {
  id: string;
  status: GoalStatus;
  revision: number;
  createdAtMs: number;
  updatedAtMs: number;
  consumedAttempts: number;
  reservedAttempts: number;
  remainingAttempts: number;
  jobCount: number;
  acceptedOutputs: number;
}
export class GoalsClient {
  constructor(private readonly client: DaemonClient) {}
  async list(options: { signal?: AbortSignal } = {}): Promise<GoalView[]> {
    return (
      await this.client.requestJson<{ goals: GoalView[] }>(
        '/api/goals',
        options,
      )
    ).goals;
  }
  create(input: GoalInput): Promise<GoalView> {
    return this.client.requestJson('/api/goals', {
      method: 'POST',
      body: input,
    });
  }
  setStatus(
    id: string,
    input: { revision: number; status: GoalStatus },
  ): Promise<GoalView> {
    return this.client.requestJson(
      `/api/goals/${encodeURIComponent(id)}/status`,
      { method: 'POST', body: input },
    );
  }
  async jobs(
    id: string,
    options: { signal?: AbortSignal } = {},
  ): Promise<AgentJob[]> {
    return (
      await this.client.requestJson<{ jobs: AgentJob[] }>(
        `/api/goals/${encodeURIComponent(id)}/jobs`,
        options,
      )
    ).jobs;
  }
}
