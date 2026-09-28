import type { DaemonClient } from './client.js';
import type { Run } from './runs.js';

/** One tool call of a run in flight. */
export interface LiveToolCard {
  stepId: string;
  toolCallId: string;
  name: string;
  argumentsPreview: string;
  argumentsTruncated: boolean;
  status: 'running' | 'success' | 'error';
  durationMs: number | null;
  resultPreview: string | null;
  truncated: boolean;
}

/** An active run as a new stream first sees it (spec §6). */
export interface SnapshotRun {
  run: Run;
  stepId: string | null;
  /** The newest part of the current step's text, at most 64 KiB. */
  text: string;
  /** UTF-16 units of the step's text before `text`. */
  textOffset: number;
  tools: LiveToolCard[];
}

interface EventBase {
  agentId: string;
  sessionId?: string;
  runId?: string;
  /** Per stream, starting at 1 with the snapshot; also the SSE id. */
  seq: number;
  at: number;
}

export type RunLifecycleEventType =
  | 'run.queued'
  | 'run.started'
  | 'run.awaiting_approval'
  | 'run.completed'
  | 'run.failed'
  | 'run.cancelled'
  | 'run.interrupted';

export type AgentEvent =
  | (EventBase & {
      type: 'stream.snapshot';
      runs: SnapshotRun[];
      approvals: unknown[];
    })
  | (EventBase & { type: 'stream.resync'; missed: number })
  | (EventBase & {
      type: 'session.created' | 'session.updated' | 'session.deleted';
    })
  | (EventBase & { type: RunLifecycleEventType; run: Run })
  | (EventBase & { type: 'run.progress'; phase: string })
  | (EventBase & { type: 'run.steered'; messageId: string; text: string })
  | (EventBase & {
      type: 'step.delta';
      stepId: string;
      /** UTF-16 offset of `text` within its step. */
      offset: number;
      text: string;
    })
  | (EventBase & {
      type: 'message.created';
      messageId: string;
      role: 'user' | 'assistant' | 'system' | 'tool';
      stepId: string | null;
    })
  | (EventBase & {
      type: 'tool.started';
      stepId: string;
      toolCallId: string;
      name: string;
      argumentsPreview: string;
      argumentsTruncated: boolean;
    })
  | (EventBase & {
      type: 'tool.finished';
      stepId: string;
      toolCallId: string;
      name: string;
      status: 'success' | 'error';
      durationMs: number;
      resultPreview: string;
      truncated: boolean;
      recovered: boolean;
    });

const LIFECYCLE: ReadonlySet<string> = new Set<RunLifecycleEventType>([
  'run.queued',
  'run.started',
  'run.awaiting_approval',
  'run.completed',
  'run.failed',
  'run.cancelled',
  'run.interrupted',
]);

export function isRunLifecycleEvent(
  event: AgentEvent,
): event is Extract<AgentEvent, { type: RunLifecycleEventType }> {
  return LIFECYCLE.has(event.type);
}

export class AgentEventsClient {
  constructor(private readonly client: DaemonClient) {}

  /** The companion's live events (spec §6): `stream.snapshot` first. The
   *  generator ends when the connection closes; reconnecting is the
   *  caller's choice. */
  async *stream(
    agentId: string,
    options: { signal?: AbortSignal } = {},
  ): AsyncGenerator<AgentEvent> {
    for await (const event of this.client.subscribe<unknown>(
      `/api/agents/${encodeURIComponent(agentId)}/events`,
      { signal: options.signal },
    )) {
      const data = event.data;
      if (data !== null && typeof data === 'object' && 'type' in data) {
        yield data as AgentEvent;
        continue;
      }
      // Not JSON, or JSON without a type: skipped, but said, so a daemon
      // and console that disagree about the wire show it (T15).
      console.warn('Skipped a malformed agent event', {
        event: event.event,
        id: event.id,
        data: typeof data === 'string' ? data.slice(0, 200) : data,
      });
    }
  }
}
