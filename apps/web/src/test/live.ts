import { vi } from 'vitest';
import type {
  AgentEvent,
  Approval,
  Run,
  RunLifecycleEventType,
  SnapshotRun,
} from '@animaOS-SWARM/sdk';

import { daemon } from '../lib/daemon-api';

/** A web run of `agent-main` in `chat:1`, queued unless overridden. */
export function runFixture(id: string, overrides: Partial<Run> = {}): Run {
  return {
    id,
    agentId: 'agent-main',
    sessionId: 'chat:1',
    source: 'web',
    sourceRef: null,
    idempotencyKey: null,
    status: 'queued',
    input: { text: 'Hello', attachmentIds: [], skill: null },
    createdAtMs: 1,
    startedAtMs: null,
    finishedAtMs: null,
    error: null,
    stop: null,
    toolsStarted: [],
    steps: [],
    usage: { promptTokens: 0, completionTokens: 0, totalTokens: 0 },
    model: 'gpt-4.1',
    provider: 'openai',
    parentRunId: null,
    replyMessageId: null,
    ...overrides,
  };
}

export function snapshotRun(
  run: Run,
  live: Partial<Omit<SnapshotRun, 'run'>> = {},
): SnapshotRun {
  return { run, stepId: null, text: '', textOffset: 0, tools: [], ...live };
}

export function snapshotEvent(
  runs: SnapshotRun[] = [],
  seq = 1,
  agentId = 'agent-main',
  approvals: Approval[] = [],
): AgentEvent {
  return { type: 'stream.snapshot', agentId, seq, at: 1, runs, approvals };
}

export function resyncEvent(
  missed: number,
  seq: number,
  agentId = 'agent-main',
): AgentEvent {
  return { type: 'stream.resync', agentId, seq, at: 1, missed };
}

export function sessionEvent(
  type: 'session.created' | 'session.updated' | 'session.deleted',
  sessionId: string,
  seq: number,
  agentId = 'agent-main',
): AgentEvent {
  return { type, agentId, sessionId, seq, at: 1 };
}

function about(run: Run, seq: number) {
  return {
    agentId: run.agentId,
    sessionId: run.sessionId,
    runId: run.id,
    seq,
    at: 1,
  };
}

export function runEvent(
  type: RunLifecycleEventType,
  run: Run,
  seq: number,
): AgentEvent {
  return { type, ...about(run, seq), run };
}

export function progressEvent(
  run: Run,
  phase: string,
  seq: number,
): AgentEvent {
  return { type: 'run.progress', ...about(run, seq), phase };
}

export function steeredEvent(
  run: Run,
  messageId: string,
  text: string,
  seq: number,
): AgentEvent {
  return { type: 'run.steered', ...about(run, seq), messageId, text };
}

export function deltaEvent(
  run: Run,
  stepId: string,
  offset: number,
  text: string,
  seq: number,
): AgentEvent {
  return { type: 'step.delta', ...about(run, seq), stepId, offset, text };
}

export function messageCreatedEvent(
  run: Run,
  messageId: string,
  role: 'user' | 'assistant' | 'system' | 'tool',
  seq: number,
): AgentEvent {
  return {
    type: 'message.created',
    ...about(run, seq),
    messageId,
    role,
    stepId: null,
  };
}

export function toolStartedEvent(
  run: Run,
  toolCallId: string,
  name: string,
  seq: number,
  argumentsPreview = '{}',
  stepId = `${run.id}:1`,
): AgentEvent {
  return {
    type: 'tool.started',
    ...about(run, seq),
    stepId,
    toolCallId,
    name,
    argumentsPreview,
    argumentsTruncated: false,
  };
}

export function toolFinishedEvent(
  run: Run,
  toolCallId: string,
  name: string,
  seq: number,
  result: {
    status?: 'success' | 'error';
    durationMs?: number;
    resultPreview?: string;
    truncated?: boolean;
    stepId?: string;
  } = {},
): AgentEvent {
  return {
    type: 'tool.finished',
    ...about(run, seq),
    stepId: result.stepId ?? `${run.id}:1`,
    toolCallId,
    name,
    status: result.status ?? 'success',
    durationMs: result.durationMs ?? 120,
    resultPreview: result.resultPreview ?? '',
    truncated: result.truncated ?? false,
    recovered: false,
  };
}

/** A pending `bash` approval of `run_1` in `chat:1`, unless overridden. */
export function approvalFixture(
  id: string,
  overrides: Partial<Approval> = {},
): Approval {
  return {
    id,
    agentId: 'agent-main',
    sessionId: 'chat:1',
    runId: 'run_1',
    toolCallId: 'call_1',
    tool: 'bash',
    class: 'exec',
    arguments: '{"command":"git status"}',
    argumentsTruncated: false,
    suggestedMatcher: { kind: 'command_prefix', value: 'git status' },
    matcherKinds: ['command_prefix', 'any'],
    createdAtMs: 1,
    expiresAtMs: 1_800_001,
    status: 'pending',
    revision: 1,
    resolution: null,
    ...overrides,
  };
}

export function approvalEvent(
  type: 'approval.requested' | 'approval.resolved',
  approval: Approval,
  seq: number,
): AgentEvent {
  return {
    type,
    agentId: approval.agentId,
    sessionId: approval.sessionId,
    runId: approval.runId,
    seq,
    at: 1,
    approval,
  };
}

async function* silentStream(
  signal: AbortSignal | undefined,
): AsyncGenerator<AgentEvent> {
  if (signal?.aborted) return;
  await new Promise<void>((resolve) =>
    signal?.addEventListener('abort', () => resolve(), { once: true }),
  );
}

/** `daemon.agentEvents` streams that stay open and silent until closed. */
export function idleAgentEvents() {
  return vi
    .spyOn(daemon, 'agentEvents')
    .mockImplementation((_agentId, options = {}) =>
      silentStream(options.signal),
    );
}

/** One `daemon.agentEvents` stream a test feeds by hand. */
export interface ScriptedStream {
  readonly agentId: string;
  readonly signal: AbortSignal | undefined;
  push(...events: AgentEvent[]): void;
  /** Ends the stream as the daemon closing it would. */
  end(): void;
  /** Fails the stream as a dropped connection or an HTTP error would. */
  fail(error: unknown): void;
}

/** Every `daemon.agentEvents` call returns a new scripted stream. */
export function scriptedAgentEvents() {
  const streams: ScriptedStream[] = [];
  vi.spyOn(daemon, 'agentEvents').mockImplementation(
    (agentId, options = {}) => {
      const queue: AgentEvent[] = [];
      let ended = false;
      // `as` (not `: T | null =`) sidesteps a TS control-flow quirk: with an
      // explicit annotation, `failure`'s type at `if (failure)` below stays
      // narrowed to the initializer's `null` and never widens back even
      // though `fail` (a sibling closure) reassigns it, so `failure.error`
      // resolves to `never`.
      let failure = null as { error: unknown } | null;
      let wake: (() => void) | null = null;
      const notify = () => {
        const resume = wake;
        wake = null;
        resume?.();
      };
      streams.push({
        agentId,
        signal: options.signal,
        push: (...events) => {
          queue.push(...events);
          notify();
        },
        end: () => {
          ended = true;
          notify();
        },
        fail: (error) => {
          failure = { error };
          notify();
        },
      });
      options.signal?.addEventListener(
        'abort',
        () => {
          ended = true;
          notify();
        },
        { once: true },
      );
      return (async function* (): AsyncGenerator<AgentEvent> {
        for (;;) {
          const next = queue.shift();
          if (next) {
            yield next;
            continue;
          }
          if (failure) throw failure.error;
          if (ended) return;
          await new Promise<void>((resolve) => {
            wake = resolve;
          });
        }
      })();
    },
  );
  return { streams, latest: () => streams[streams.length - 1] };
}

export function skillEvent(
  seq: number,
  slug: string | null = 'notes',
  agentId = 'agent-main',
): AgentEvent {
  return { type: 'skill.updated', agentId, seq, at: 1, slug, draftId: null };
}

export function automationEvent(
  seq: number,
  scheduleId = 'schedule-1',
  agentId = 'agent-main',
): AgentEvent {
  return {
    type: 'automation.updated',
    agentId,
    seq,
    at: 1,
    scheduleId,
    deleted: false,
  };
}
