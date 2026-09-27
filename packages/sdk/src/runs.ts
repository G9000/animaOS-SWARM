import type { DaemonClient } from './client.js';

export type RunStatus =
  | 'queued'
  | 'running'
  | 'awaiting_approval'
  | 'completed'
  | 'failed'
  | 'cancelled'
  | 'interrupted';

export type RunSource =
  | 'web'
  | 'api'
  | 'telegram'
  | 'schedule'
  | 'job'
  | 'delegation'
  | 'peer';

export interface RunTokenUsage {
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
}

/** A ledger run (spec §4.1). */
export interface Run {
  id: string;
  agentId: string;
  sessionId: string;
  source: RunSource;
  sourceRef: string | null;
  /** The `Idempotency-Key` the run was accepted or started with, so a client
   *  matches its own sends; `null` for a run without one. */
  idempotencyKey: string | null;
  status: RunStatus;
  input: { text: string; attachmentIds: string[]; skill: string | null };
  createdAtMs: number;
  startedAtMs: number | null;
  finishedAtMs: number | null;
  error: { code: string; message: string } | null;
  stop: { requestedAtMs: number } | null;
  toolsStarted: string[];
  steps: { stepId: string; usage: RunTokenUsage }[];
  usage: RunTokenUsage;
  model: string;
  provider: string | null;
  parentRunId: string | null;
  /** The committed final reply once the run completed. */
  replyMessageId: string | null;
}

/** `queue` waits behind the session's earlier messages; `steer` joins its
 *  active run before the next model call (spec §4.2, §4.7). */
export type RunMode = 'queue' | 'steer';

export interface StartRunInput {
  text: string;
  attachmentIds?: string[];
  skill?: string;
  mode?: RunMode;
}

export interface StartRunResult {
  run: Run;
  /** Present when the message joined the session's active run. */
  steer?: { status: 'pending' };
}

const TERMINAL: ReadonlySet<RunStatus> = new Set([
  'completed',
  'failed',
  'cancelled',
  'interrupted',
]);

export function isTerminalRunStatus(status: RunStatus): boolean {
  return TERMINAL.has(status);
}

export class RunsClient {
  constructor(private readonly client: DaemonClient) {}

  /** Accepts a message into a session (spec §4.2). The same key within 24
   *  hours returns the original run and creates nothing. */
  async start(
    agentId: string,
    sessionId: string,
    input: StartRunInput,
    options: { idempotencyKey: string; signal?: AbortSignal },
  ): Promise<StartRunResult> {
    return this.client.requestJson<StartRunResult>(
      `${sessionPath(agentId, sessionId)}/runs`,
      {
        method: 'POST',
        body: input,
        headers: { 'idempotency-key': options.idempotencyKey },
        signal: options.signal,
      },
    );
  }

  /** Stops a run (spec §4.6); stopping a finished run changes nothing. */
  async stop(agentId: string, runId: string): Promise<Run> {
    const response = await this.client.requestJson<{ run: Run }>(
      `${runPath(agentId, runId)}/stop`,
      { method: 'POST' },
    );
    return response.run;
  }

  async get(
    agentId: string,
    runId: string,
    options: { signal?: AbortSignal } = {},
  ): Promise<Run> {
    const response = await this.client.requestJson<{ run: Run }>(
      runPath(agentId, runId),
      { signal: options.signal },
    );
    return response.run;
  }

  /** The session's runs the daemon's ledger holds, newest first. */
  async listForSession(
    agentId: string,
    sessionId: string,
    options: { limit?: number; signal?: AbortSignal } = {},
  ): Promise<Run[]> {
    const query =
      options.limit !== undefined ? `?limit=${String(options.limit)}` : '';
    const response = await this.client.requestJson<{ runs: Run[] }>(
      `${sessionPath(agentId, sessionId)}/runs${query}`,
      { signal: options.signal },
    );
    return response.runs;
  }
}

function sessionPath(agentId: string, sessionId: string): string {
  return `/api/agents/${encodeURIComponent(agentId)}/sessions/${encodeURIComponent(sessionId)}`;
}

function runPath(agentId: string, runId: string): string {
  return `/api/agents/${encodeURIComponent(agentId)}/runs/${encodeURIComponent(runId)}`;
}
