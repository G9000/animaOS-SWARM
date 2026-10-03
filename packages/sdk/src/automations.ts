import type { DaemonClient } from './client.js';

/** When an automation fires (spec §9.1). Daily and cron times are wall-clock
 *  times in their IANA `timeZone`; `once` fires one time and then turns the
 *  automation off. */
export type AutomationTrigger =
  | { type: 'interval'; intervalMs: number }
  | { type: 'daily'; hour: number; minute: number; timeZone: string }
  | { type: 'cron'; expression: string; timeZone: string }
  | { type: 'once'; atMs: number };

/** When it may fire: `HH:MM` wall times, days 0 (Sunday) to 6. An `end`
 *  before `start` runs overnight from the day it starts. */
export interface ActiveHours {
  start: string;
  end: string;
  days: number[];
  timeZone: string;
}

/** `workspace`: the automation's own thread. */
export type AutomationTarget =
  | { type: 'workspace' }
  | { type: 'connector'; connectorId: string };

/** Who made it; the companion's names the tool call (for its notice card). */
export type AutomationCreator =
  | { kind: 'owner' }
  | {
      kind: 'agent';
      agentId: string;
      sessionId: string;
      runId: string;
      toolCallId: string;
    };

export interface AutomationOutcome {
  /** `error` for a failure (the history says `failed`). */
  status: 'silent' | 'spoke' | 'error' | 'stopped';
  occurredAtMs: number;
  errorCode: string | null;
}

export interface AutomationCounters {
  runs: number;
  failures: number;
  consecutiveFailures: number;
}

export interface Automation {
  id: string;
  agentId: string;
  /** Written by the owner or the companion: show it as text only. */
  name: string;
  prompt: string;
  trigger: AutomationTrigger;
  activeHours: ActiveHours | null;
  enabled: boolean;
  target: AutomationTarget;
  nextDueAtMs: number;
  lastFiredAtMs: number | null;
  lastOutcome: AutomationOutcome | null;
  /** Its latest occurrence has no outcome yet. */
  running: boolean;
  createdBy: AutomationCreator;
  preset: 'heartbeat' | null;
  counters: AutomationCounters;
  importIdempotencyKey: string | null;
  createdAtMs: number;
  updatedAtMs: number;
}

export interface AutomationInput {
  prompt: string;
  trigger: AutomationTrigger;
  /** The automation's own thread when absent. */
  target?: AutomationTarget;
  /** From the prompt's first line when absent. */
  name?: string;
  activeHours?: ActiveHours;
  enabled?: boolean;
}

/** The heartbeat preset (spec §9.2): every 30 minutes from 08:00 to 22:00
 *  in `timeZone`; any other field replaces the preset's. */
export interface HeartbeatInput {
  timeZone: string;
  prompt?: string;
  name?: string;
  target?: AutomationTarget;
}

export interface AutomationPatch {
  prompt?: string;
  trigger?: AutomationTrigger;
  target?: AutomationTarget;
  enabled?: boolean;
  name?: string;
  /** `null` clears them. */
  activeHours?: ActiveHours | null;
}

export type AutomationRunOutcome = 'silent' | 'spoke' | 'failed' | 'stopped';

/** One occurrence (spec §9.1). */
export interface AutomationRun {
  id: string;
  scheduleId: string;
  agentId: string;
  firedAtMs: number;
  finishedAtMs: number;
  outcome: AutomationRunOutcome;
  runId: string | null;
  sessionId: string | null;
  errorCode: string | null;
  /** Run now, not the trigger. */
  manual: boolean;
}

/** The daemon's limits (spec §9.3, §16). */
export const MAX_AUTOMATIONS_PER_AGENT = 20;
export const MAX_AUTOMATION_HISTORY = 50;
export const MAX_AUTOMATION_NAME_CHARS = 80;
export const AUTOMATION_PREVIEW_RUNS = 3;

function collection(agentId: string): string {
  return `/api/agents/${encodeURIComponent(agentId)}/schedules`;
}

function item(agentId: string, id: string): string {
  return `${collection(agentId)}/${encodeURIComponent(id)}`;
}

function only<T extends object>(
  value: T,
  keys: readonly (keyof T)[],
): Partial<T> {
  const picked: Partial<T> = {};
  for (const key of keys)
    if (value[key] !== undefined) picked[key] = value[key];
  return picked;
}

const INPUT_KEYS = [
  'prompt',
  'trigger',
  'target',
  'name',
  'activeHours',
  'enabled',
] as const;
const PATCH_KEYS = [
  'prompt',
  'trigger',
  'target',
  'enabled',
  'name',
  'activeHours',
] as const;

export class AutomationsClient {
  constructor(private readonly client: DaemonClient) {}

  /** The agent's automations, oldest first. */
  async list(
    agentId: string,
    options: { signal?: AbortSignal } = {},
  ): Promise<Automation[]> {
    const response = await this.client.requestJson<{
      schedules: Automation[];
    }>(collection(agentId), { signal: options.signal });
    return response.schedules;
  }

  async create(agentId: string, input: AutomationInput): Promise<Automation> {
    const response = await this.client.requestJson<{ schedule: Automation }>(
      collection(agentId),
      { method: 'POST', body: only(input, INPUT_KEYS) },
    );
    return response.schedule;
  }

  async createHeartbeat(
    agentId: string,
    input: HeartbeatInput,
  ): Promise<Automation> {
    const body = {
      preset: 'heartbeat',
      ...only(input, ['timeZone', 'prompt', 'name', 'target'] as const),
    };
    const response = await this.client.requestJson<{ schedule: Automation }>(
      collection(agentId),
      { method: 'POST', body },
    );
    return response.schedule;
  }

  async update(
    agentId: string,
    id: string,
    patch: AutomationPatch,
  ): Promise<Automation> {
    const response = await this.client.requestJson<{ schedule: Automation }>(
      item(agentId, id),
      { method: 'PATCH', body: only(patch, PATCH_KEYS) },
    );
    return response.schedule;
  }

  async remove(agentId: string, id: string): Promise<void> {
    await this.client.requestJson(item(agentId, id), { method: 'DELETE' });
  }

  /** Fires it now; its due time and switch stay (spec §9.2). */
  async runNow(agentId: string, id: string): Promise<Automation> {
    const response = await this.client.requestJson<{ schedule: Automation }>(
      `${item(agentId, id)}/run`,
      { method: 'POST' },
    );
    return response.schedule;
  }

  /** Its latest occurrences, newest first (at most 50). */
  async history(
    agentId: string,
    id: string,
    options: { limit?: number; signal?: AbortSignal } = {},
  ): Promise<AutomationRun[]> {
    const query =
      options.limit === undefined ? '' : `?limit=${String(options.limit)}`;
    const response = await this.client.requestJson<{ runs: AutomationRun[] }>(
      `${item(agentId, id)}/history${query}`,
      { signal: options.signal },
    );
    return response.runs;
  }

  /** The next fire times the daemon computes (spec §9.2): three, or one for
   *  `once`. */
  async preview(
    input: { trigger: AutomationTrigger; activeHours?: ActiveHours },
    options: { signal?: AbortSignal } = {},
  ): Promise<number[]> {
    const response = await this.client.requestJson<{ nextRuns: number[] }>(
      '/api/schedules/preview',
      {
        method: 'POST',
        body: only(input, ['trigger', 'activeHours'] as const),
        signal: options.signal,
      },
    );
    return response.nextRuns;
  }
}
