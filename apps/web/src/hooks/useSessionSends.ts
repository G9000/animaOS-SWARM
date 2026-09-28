import { useEffect, useState, useSyncExternalStore } from 'react';
import {
  DaemonConnectionError,
  DaemonHttpError,
  type RunMode,
  type StartRunResult,
} from '@animaOS-SWARM/sdk';

import { daemon } from '../lib/daemon-api';

/** Automatic retries of a send that may not have arrived (spec §15.5). */
export const SEND_RETRY_DELAYS_MS: readonly number[] = [1_000, 2_000, 4_000];

/** A message on its way to a session's runs route (spec §4.2). */
export interface SessionSend {
  /** The Idempotency-Key; also the committed message's `clientRequestId`. */
  key: string;
  agentId: string;
  sessionId: string;
  /** The chat state the send belongs to (ViewHarness's `chatKey`). */
  conversation: string;
  text: string;
  mode: RunMode;
  /** Telegram errors are scrubbed of bot tokens before they are shown. */
  telegram: boolean;
  createdAtMs: number;
  /** Failed attempts so far. */
  failures: number;
  /** Set once a steer joined a run, until that run applies or ends it. */
  steeringRunId: string | null;
}

export type NewSessionSend = Omit<
  SessionSend,
  'createdAtMs' | 'failures' | 'steeringRunId'
>;

/** A failure a retry may fix: the request may not have reached the daemon,
 *  or a gateway stopped waiting. Any other answer is the daemon's last word. */
export function isRetryableSendError(error: unknown): boolean {
  if (error instanceof DaemonConnectionError) return true;
  return (
    error instanceof DaemonHttpError &&
    (error.status === 408 || error.status === 502 || error.status === 504)
  );
}

export interface SessionSendCallbacks {
  onAccepted: (send: SessionSend, result: StartRunResult) => void;
  onFailed: (send: SessionSend, error: unknown) => void;
  /** A send still unaccepted when the page last closed (spec §15.5, S3b-A):
   *  handed back once, on the next load, for the recovery panel to offer. */
  onRestore?: (send: PersistedSend) => void;
}

function laneOf(send: Pick<SessionSend, 'agentId' | 'sessionId'>): string {
  return `${send.agentId}\u0000${send.sessionId}`;
}

/** Enough of an unaccepted send to offer it back in the recovery panel:
 *  never its mode, telegram flag, retry count, or steer state. */
export interface PersistedSend {
  /** Reused as the resend's Idempotency-Key, so it joins rather than
   *  doubles a request that did reach the daemon. */
  key: string;
  text: string;
  /** The chat it belongs to (ViewHarness's chatKey). */
  conversation: string;
  createdAtMs: number;
}

const PENDING_SENDS_STORAGE_KEY = 'animaos.pendingSends';

function isPersistedSend(value: unknown): value is PersistedSend {
  if (typeof value !== 'object' || value === null) return false;
  const item = value as Record<string, unknown>;
  return (
    typeof item.key === 'string' &&
    typeof item.text === 'string' &&
    typeof item.conversation === 'string' &&
    typeof item.createdAtMs === 'number'
  );
}

function loadPendingSends(): PersistedSend[] {
  try {
    const raw = window.sessionStorage.getItem(PENDING_SENDS_STORAGE_KEY);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed.filter(isPersistedSend) : [];
  } catch {
    return [];
  }
}

function savePendingSends(sends: readonly PersistedSend[]): void {
  try {
    if (sends.length === 0)
      window.sessionStorage.removeItem(PENDING_SENDS_STORAGE_KEY);
    else
      window.sessionStorage.setItem(
        PENDING_SENDS_STORAGE_KEY,
        JSON.stringify(sends),
      );
  } catch {
    // Storage is full or blocked: unaccepted sends live in memory only.
  }
}

function toPersisted(send: SessionSend): PersistedSend {
  return {
    key: send.key,
    text: send.text,
    conversation: send.conversation,
    createdAtMs: send.createdAtMs,
  };
}

/** Sends in flight: one request at a time per session, so the daemon
 *  accepts a session's messages in the order they were written (spec §4.3). */
export class SendQueue {
  private sends: SessionSend[] = [];
  private readonly listeners = new Set<() => void>();
  private readonly busy = new Set<string>();
  /** Retries waiting to fire: each holds its send's lane busy. */
  private readonly timers = new Map<number, { lane: string; key: string }>();
  private closed = false;
  private restored = false;

  constructor(private callbacks: SessionSendCallbacks) {}

  setCallbacks(callbacks: SessionSendCallbacks): void {
    this.callbacks = callbacks;
  }

  readonly snapshot = (): readonly SessionSend[] => this.sends;

  readonly subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  readonly send = (input: NewSessionSend): void => {
    if (this.sends.some((item) => item.key === input.key)) return;
    this.set([
      ...this.sends,
      { ...input, createdAtMs: Date.now(), failures: 0, steeringRunId: null },
    ]);
    this.pump(laneOf(input));
  };

  /** Drops a send's bubble: accepted, failed, or its steer applied. */
  readonly settle = (key: string): void => {
    if (this.sends.some((item) => item.key === key))
      this.set(this.sends.filter((item) => item.key !== key));
  };

  /** The owner takes back a send the daemon has not accepted (spec §15.3,
   *  S3b-I): it leaves the queue, its retry with it, and is returned for the
   *  recovery panel. A request already on its way may still land; its key,
   *  kept with the text, makes a resend join rather than double it. A steer
   *  the daemon took is not the page's to cancel. */
  readonly cancel = (key: string): SessionSend | null => {
    const send = this.current(key);
    if (!send || send.steeringRunId !== null) return null;
    const lane = laneOf(send);
    let retryCleared = false;
    for (const [timer, pending] of this.timers) {
      if (pending.key !== key) continue;
      window.clearTimeout(timer);
      this.timers.delete(timer);
      retryCleared = true;
    }
    this.settle(key);
    if (retryCleared) {
      this.busy.delete(lane);
      this.pump(lane);
    }
    return send;
  };

  /** Forgets a deleted companion's sends. */
  readonly forgetAgent = (agentId: string): void => {
    if (this.sends.some((item) => item.agentId === agentId))
      this.set(this.sends.filter((item) => item.agentId !== agentId));
  };

  open(): void {
    this.closed = false;
    // Once per instance (a page load), not on React's StrictMode
    // close-then-reopen: the first read already emptied storage.
    if (!this.restored) {
      this.restored = true;
      const leftOver = loadPendingSends();
      if (leftOver.length > 0) {
        savePendingSends([]);
        for (const send of leftOver) this.callbacks.onRestore?.(send);
      }
    }
    for (const lane of new Set(this.sends.map(laneOf))) this.pump(lane);
  }

  /** Stops retrying; `open` resumes (React may close and reopen on mount). */
  close(): void {
    this.closed = true;
    for (const [timer, { lane }] of this.timers) {
      window.clearTimeout(timer);
      this.busy.delete(lane);
    }
    this.timers.clear();
  }

  private set(next: SessionSend[]): void {
    this.sends = next;
    savePendingSends(next.map(toPersisted));
    for (const listener of this.listeners) listener();
  }

  private current(key: string): SessionSend | undefined {
    return this.sends.find((item) => item.key === key);
  }

  private patch(key: string, patch: Partial<SessionSend>): void {
    this.set(
      this.sends.map((item) =>
        item.key === key ? { ...item, ...patch } : item,
      ),
    );
  }

  private pump(lane: string): void {
    if (this.closed || this.busy.has(lane)) return;
    const next = this.sends.find(
      (item) => laneOf(item) === lane && item.steeringRunId === null,
    );
    if (next) void this.attempt(next);
  }

  private async attempt(send: SessionSend): Promise<void> {
    const lane = laneOf(send);
    this.busy.add(lane);
    let result: StartRunResult;
    try {
      result = await daemon.startRun(
        send.agentId,
        send.sessionId,
        { text: send.text, mode: send.mode },
        send.key,
      );
    } catch (error) {
      this.failed(send, error);
      return;
    }
    this.busy.delete(lane);
    // A closed queue keeps the send: reopened, it sends the same key again
    // and the daemon answers with the run it already accepted.
    if (this.closed) return;
    // A send forgotten meanwhile (its companion was deleted) is not reported.
    if (this.current(send.key)) {
      if (result.steer) this.patch(send.key, { steeringRunId: result.run.id });
      else this.settle(send.key);
      this.callbacks.onAccepted(send, result);
    }
    this.pump(lane);
  }

  private failed(send: SessionSend, error: unknown): void {
    const lane = laneOf(send);
    if (this.closed || !this.current(send.key)) {
      this.busy.delete(lane);
      this.pump(lane);
      return;
    }
    const failures = send.failures + 1;
    if (
      isRetryableSendError(error) &&
      failures <= SEND_RETRY_DELAYS_MS.length
    ) {
      this.patch(send.key, { failures });
      const timer = window.setTimeout(
        () => {
          this.timers.delete(timer);
          this.busy.delete(lane);
          const latest = this.current(send.key);
          if (latest && !this.closed) void this.attempt(latest);
          else this.pump(lane);
        },
        SEND_RETRY_DELAYS_MS[failures - 1],
      );
      this.timers.set(timer, { lane, key: send.key });
      return;
    }
    this.busy.delete(lane);
    this.settle(send.key);
    this.callbacks.onFailed({ ...send, failures: failures - 1 }, error);
    this.pump(lane);
  }
}

/** The page's sends (spec §15.5): each session's messages in order, retried
 *  with their key, shown as pending bubbles until the daemon accepts them. */
export function useSessionSends(callbacks: SessionSendCallbacks) {
  const [queue] = useState(() => new SendQueue(callbacks));
  useEffect(() => {
    queue.setCallbacks(callbacks);
  });
  useEffect(() => {
    queue.open();
    return () => queue.close();
  }, [queue]);
  const sends = useSyncExternalStore(queue.subscribe, queue.snapshot);
  return {
    sends,
    send: queue.send,
    settle: queue.settle,
    cancel: queue.cancel,
    forgetAgent: queue.forgetAgent,
  };
}
