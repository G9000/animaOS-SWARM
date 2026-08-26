// Typed client for the anima-daemon server (hosts/rust-daemon).
// Single-agent UX on top of the daemon's multi-agent runtime:
// the UI drives the FIRST agent returned by GET /api/agents.
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

/** Model suggestions keyed by daemon provider id. */
export const MODEL_SUGGESTIONS: Record<string, string[]> = {
  ...PROVIDER_MODELS,
  deterministic: ['deterministic'],
};

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`/api${path}`, {
    headers: init?.body ? { 'content-type': 'application/json' } : undefined,
    ...init,
  });
  if (!response.ok) {
    let message = `daemon request failed (${response.status})`;
    try {
      const body = (await response.json()) as { error?: string; message?: string };
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

export const daemon = {
  health: () => request<{ status: string }>('/health'),

  listProviders: () => request<{ providers: DaemonProvider[] }>('/providers'),

  listAgents: () => request<{ agents: DaemonSnapshot[] }>('/agents'),

  getAgent: (id: string) => request<{ agent: DaemonSnapshot }>(`/agents/${id}`),

  /** name and model are required by the daemon. */
  createAgent: (input: {
    name: string;
    model: string;
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
  updateAgent: (
    id: string,
    patch: { name?: string; model?: string; provider?: string; system?: string }
  ) =>
    request<{ agent: DaemonSnapshot }>(`/agents/${id}`, {
      method: 'PATCH',
      body: JSON.stringify(patch),
    }),

  /** The single-agent chat loop: user text in, task result out. */
  runAgent: (id: string, text: string, metadata?: Record<string, unknown>) =>
    request<{ agent: DaemonSnapshot; result: DaemonRunResult }>(
      `/agents/${id}/run`,
      { method: 'POST', body: JSON.stringify({ text, ...(metadata ? { metadata } : {}) }) }
    ),
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
        if (m.role === 'user' && m.content.metadata?.kind === 'checkin') return false;
        if (m.role === 'assistant' && m.content.text.trim() === 'CHECKIN_OK') return false;
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
