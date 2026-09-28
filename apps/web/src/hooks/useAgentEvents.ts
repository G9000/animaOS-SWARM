import { useCallback, useEffect, useRef, useSyncExternalStore } from 'react';
import { DaemonHttpError, type AgentEvent } from '@animaOS-SWARM/sdk';

import { daemon } from '../lib/daemon-api';
import {
  EMPTY_LIVE_STATE,
  applyEvent,
  trimCommittedRun,
  type LiveState,
} from '../lib/session-events';

/** Reconnect back-off (spec §15.5): from 1 to 30 seconds, with jitter. */
export const STREAM_RETRY_MIN_MS = 1_000;
export const STREAM_RETRY_MAX_MS = 30_000;

/**
 * Ruling 3 (M3 pre-flight audit M21): a resync means the buffer overflowed
 * and the reconnect is otherwise free (it always lands a fresh snapshot), so
 * one or two in a row still reconnect at once. But a stream that keeps
 * resyncing without ever delivering a single normal event in between is
 * flapping, not catching up: once this many consecutive resyncs have each
 * reconnected immediately, the next one falls back to the same back-off as
 * any other drop, instead of hammering the daemon every time.
 */
export const RESYNC_BACKOFF_AFTER = 3;

/**
 * Fix round 1 (Minor 1): `stream.snapshot` alone doesn't prove a connection
 * is healthy — a stream that gets its snapshot and then drops immediately,
 * over and over, must still climb the back-off instead of retrying every
 * `STREAM_RETRY_MIN_MS` forever. The back-off counter (`failures`) only
 * resets once the connection proves itself: it delivers a real event (not
 * the snapshot, not a resync), or it simply stays open this long (covers a
 * quiet session past its snapshot, including through the daemon's SSE
 * keep-alive comments, which never reach the generator as events).
 */
export const STREAM_HEALTHY_AFTER_MS = 10_000;

export type AgentStreamStatus =
  | 'connecting'
  | 'open'
  | 'reconnecting'
  /** The daemon has no event stream (it predates M3): views poll instead. */
  | 'unsupported';

/** What the stream itself publishes; the hook adds `trimFinishedRun`. */
interface StreamSnapshot {
  status: AgentStreamStatus;
  state: LiveState;
}

export interface AgentEventsView extends StreamSnapshot {
  /** Drops a finished run's steps and tool cards once its messages are
   *  committed (S3b-B). */
  trimFinishedRun: (runId: string) => void;
}

/** The wait before reconnect `attempt` (0-based): half to all of a doubling
 *  step, never under the floor or over the cap. */
export function retryDelay(
  attempt: number,
  random: () => number = Math.random,
): number {
  const step = Math.min(
    STREAM_RETRY_MAX_MS,
    STREAM_RETRY_MIN_MS * 2 ** attempt,
  );
  return Math.max(
    STREAM_RETRY_MIN_MS,
    Math.round(step / 2 + random() * (step / 2)),
  );
}

function nextFrame(callback: () => void): () => void {
  if (typeof window.requestAnimationFrame === 'function') {
    const handle = window.requestAnimationFrame(callback);
    return () => window.cancelAnimationFrame(handle);
  }
  const handle = window.setTimeout(callback, 16);
  return () => window.clearTimeout(handle);
}

/** Ruling 2 (audit M15): a 404 from the events route alone cannot tell an
 *  unknown or deleted agent apart from a daemon that predates this stream's
 *  route (`GET /api/agents/{id}/events` did not exist yet). Probing the
 *  agent itself, exactly as `SessionsClient.list` does for `DaemonTooOldError`,
 *  resolves it: the agent existing means the route is the thing missing. */
async function agentExists(agentId: string): Promise<boolean> {
  try {
    await daemon.getAgent(agentId);
    return true;
  } catch {
    return false;
  }
}

const streams = new Map<string, AgentStream>();

/** One companion's event stream, shared by every view that watches it. */
class AgentStream {
  private status: AgentStreamStatus = 'connecting';
  private state: LiveState = EMPTY_LIVE_STATE;
  private published: StreamSnapshot = {
    status: 'connecting',
    state: EMPTY_LIVE_STATE,
  };
  private readonly listeners = new Set<() => void>();
  private readonly eventListeners = new Set<(event: AgentEvent) => void>();
  private holders = 0;
  private controller: AbortController | null = null;
  private retryTimer: number | undefined;
  private healthyTimer: number | undefined;
  private cancelFrame: (() => void) | null = null;
  private failures = 0;
  /** Consecutive resync-only connections (Ruling 3); reset by any event that
   *  is not itself `stream.snapshot` or `stream.resync`. */
  private consecutiveResyncs = 0;

  constructor(readonly agentId: string) {}

  readonly snapshot = (): StreamSnapshot => this.published;

  readonly subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  listen(listener: (event: AgentEvent) => void): () => void {
    this.eventListeners.add(listener);
    return () => {
      this.eventListeners.delete(listener);
    };
  }

  /** Drops a finished run's steps and tool cards once a view reports its
   *  messages are committed (S3b-B): a no-op once already trimmed. */
  trim(runId: string): void {
    const next = trimCommittedRun(this.state, runId);
    if (next === this.state) return;
    this.state = next;
    this.publish();
  }

  retain(): void {
    this.holders += 1;
    if (this.holders > 1) return;
    streams.set(this.agentId, this);
    this.connect();
  }

  release(): void {
    this.holders -= 1;
    if (this.holders > 0) return;
    this.controller?.abort();
    this.controller = null;
    if (this.retryTimer !== undefined) window.clearTimeout(this.retryTimer);
    this.retryTimer = undefined;
    this.clearHealthyTimer();
    this.cancelFrame?.();
    this.cancelFrame = null;
    if (streams.get(this.agentId) === this) streams.delete(this.agentId);
  }

  private connect(): void {
    this.retryTimer = undefined;
    const controller = new AbortController();
    this.controller = controller;
    this.armHealthyTimer(controller);
    void this.read(controller);
  }

  /** Minor 1: resets `failures` once this connection has stayed open long
   *  enough to count as healthy, even without a qualifying event (a quiet
   *  session past its snapshot). Superseded or dropped connections never
   *  fire this against the wrong attempt: `connect` re-arms it, and
   *  `release` clears it. */
  private armHealthyTimer(controller: AbortController): void {
    this.clearHealthyTimer();
    this.healthyTimer = window.setTimeout(() => {
      this.healthyTimer = undefined;
      if (this.controller === controller) this.failures = 0;
    }, STREAM_HEALTHY_AFTER_MS);
  }

  private clearHealthyTimer(): void {
    if (this.healthyTimer !== undefined) window.clearTimeout(this.healthyTimer);
    this.healthyTimer = undefined;
  }

  private async read(controller: AbortController): Promise<void> {
    let resync = false;
    try {
      for await (const event of daemon.agentEvents(this.agentId, {
        signal: controller.signal,
      })) {
        if (controller.signal.aborted) return;
        if (event.type === 'stream.snapshot') {
          this.status = 'open';
        } else if (event.type !== 'stream.resync') {
          // A real event (not the connection handshake, not another
          // resync): the stream is keeping up. Ruling 3's streak resets,
          // and so does the back-off counter (Minor 1) — no need to wait
          // out `STREAM_HEALTHY_AFTER_MS` when the stream already proved
          // itself.
          this.consecutiveResyncs = 0;
          this.failures = 0;
        }
        this.state = applyEvent(this.state, event);
        this.publish();
        for (const listener of this.eventListeners) {
          try {
            listener(event);
          } catch (error) {
            console.error(error);
          }
        }
        if (event.type === 'stream.resync') {
          resync = true;
          this.consecutiveResyncs += 1;
          break;
        }
      }
    } catch (error) {
      if (controller.signal.aborted) return;
      if (error instanceof DaemonHttpError && error.status === 404) {
        // Ruling 2: tell an unsupported daemon apart from an agent that is
        // simply unknown or was deleted before probing further.
        const exists = await agentExists(this.agentId);
        if (controller.signal.aborted || this.controller !== controller) return;
        if (exists) {
          this.controller = null;
          this.status = 'unsupported';
          this.publish();
          return;
        }
        // The agent itself is missing: treat like any other drop below,
        // rather than declaring the daemon too old.
      } else {
        // Ruling 2: never swallow a failure silently. 403/429 and any other
        // non-404 error still fall through to the back-off below instead of
        // reconnecting at once.
        console.warn(
          `useAgentEvents: event stream for agent "${this.agentId}" failed`,
          error,
        );
      }
    }
    if (controller.signal.aborted || this.controller !== controller) return;
    controller.abort();
    this.controller = null;
    // A fresh snapshot replaces whatever a lagging stream missed (spec §6),
    // unless Ruling 3's streak of resyncs without a normal event has grown
    // too long: then it falls through to the normal back-off below instead.
    if (resync && this.consecutiveResyncs <= RESYNC_BACKOFF_AFTER) {
      this.connect();
      return;
    }
    this.status = 'reconnecting';
    this.publish();
    this.retryTimer = window.setTimeout(
      () => this.connect(),
      retryDelay(this.failures),
    );
    this.failures += 1;
  }

  /** Views re-render at most once per animation frame (spec §15.2). */
  private publish(): void {
    if (this.cancelFrame) return;
    this.cancelFrame = nextFrame(() => {
      this.cancelFrame = null;
      this.published = { status: this.status, state: this.state };
      for (const listener of this.listeners) listener();
    });
  }
}

function streamFor(agentId: string): AgentStream {
  let stream = streams.get(agentId);
  if (!stream) {
    stream = new AgentStream(agentId);
    streams.set(agentId, stream);
  }
  return stream;
}

const IDLE: StreamSnapshot = { status: 'connecting', state: EMPTY_LIVE_STATE };
const subscribeNowhere = () => () => undefined;
const idleSnapshot = () => IDLE;

/**
 * The companion's live events (spec §15.5 `useAgentEvents`): one SSE
 * connection per companion shared by every caller, reconnecting with
 * back-off. `onEvent` sees each event as it arrives; the returned state
 * changes at most once per animation frame.
 */
export function useAgentEvents(
  agentId: string | null,
  onEvent?: (event: AgentEvent) => void,
): AgentEventsView {
  const stream = agentId ? streamFor(agentId) : null;
  const onEventRef = useRef(onEvent);
  useEffect(() => {
    onEventRef.current = onEvent;
  });
  useEffect(() => {
    if (!stream) return;
    const stopListening = stream.listen((event) => onEventRef.current?.(event));
    stream.retain();
    return () => {
      stopListening();
      stream.release();
    };
  }, [stream]);
  const snapshot = useSyncExternalStore(
    stream ? stream.subscribe : subscribeNowhere,
    stream ? stream.snapshot : idleSnapshot,
  );
  const trimFinishedRun = useCallback(
    (runId: string) => stream?.trim(runId),
    [stream],
  );
  return { ...snapshot, trimFinishedRun };
}
