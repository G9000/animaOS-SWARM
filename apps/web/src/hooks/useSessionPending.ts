import { useEffect, useMemo, useRef, useState } from 'react';
import type { SessionMessage } from '@animaOS-SWARM/sdk';

import type { LiveRun } from '../lib/session-events';
import type { PendingBubble } from '../lib/transcript';
import type { SessionSend } from './useSessionSends';

export interface SessionPendingOptions {
  /** Every send in flight (the page's send queue). */
  sends: readonly SessionSend[];
  /** Drops a send from the queue. */
  settle: (key: string) => void;
  /** The open session, or null for a new chat. */
  session: { agentId: string; sessionId: string } | null;
  /** The open session's loaded history. */
  messages: readonly SessionMessage[];
  /** The open session's runs, from its stream and ledger. */
  runs: readonly LiveRun[];
  /** Reads the open session's newest messages again. */
  refreshMessages: () => Promise<void>;
  /** A steer that no run took and no history holds: its text goes to the
   *  composer's recovery panel. */
  onRecover: (send: SessionSend) => void;
}

const NO_RUNS: ReadonlySet<string> = new Set();

/** A reply that ended without being able to take a steer. */
function endedBadly(status: string): boolean {
  return status === 'failed' || status === 'interrupted';
}

/** The keys of steers the runs they joined show as taken (`run.steered`),
 *  matched by text in the order they were sent. */
function shownSteers(
  sends: readonly SessionSend[],
  runs: readonly LiveRun[],
): Set<string> {
  const taken = new Map<string, string[]>();
  for (const live of runs)
    if (live.steers.length > 0)
      taken.set(
        live.run.id,
        live.steers.map((steer) => steer.text),
      );
  const shown = new Set<string>();
  for (const send of sends) {
    const texts = send.steeringRunId ? taken.get(send.steeringRunId) : null;
    const index = texts ? texts.indexOf(send.text) : -1;
    if (!texts || index < 0) continue;
    texts.splice(index, 1);
    shown.add(send.key);
  }
  return shown;
}

/**
 * The open session's sends as pending bubbles (spec §15.5), and the steers'
 * lifecycle (M3 audit I3, web part). A steer's text stays on screen until
 * history holds its message (`clientRequestId`, the send's key) or it shows
 * as a run of its own (the daemon queues a steer its reply did not take,
 * and makes one it could not queue an `interrupted` run to send again).
 * While the reply it joined shows it as taken, the run shows the text and
 * the bubble steps aside. A reconnect settles nothing. If the reply ends
 * `failed` or `interrupted` and history, read again afterwards, still does
 * not hold it, the text moves to the recovery panel.
 */
export function useSessionPending({
  sends,
  settle,
  session,
  messages,
  runs,
  refreshMessages,
  onRecover,
}: SessionPendingOptions): PendingBubble[] {
  const agentId = session?.agentId ?? null;
  const sessionId = session?.sessionId ?? null;
  const open = useMemo(
    () =>
      sends.filter(
        (item) => item.agentId === agentId && item.sessionId === sessionId,
      ),
    [sends, agentId, sessionId],
  );

  /** The runs each steer's session had when it was sent: none of them is
   *  the run the steer may become. */
  const baselinesRef = useRef(new Map<string, ReadonlySet<string>>());
  /** Steers whose failed reply prompted a read of history, by how far
   *  that went. */
  const checksRef = useRef(new Map<string, 'reading' | 'read' | 'recovered'>());
  const [checksLanded, setChecksLanded] = useState(0);
  const onRecoverRef = useRef(onRecover);
  useEffect(() => {
    onRecoverRef.current = onRecover;
  });
  const mountedRef = useRef(true);
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  useEffect(() => {
    const baselines = baselinesRef.current;
    const checks = checksRef.current;
    const queued = new Set(sends.map((item) => item.key));
    for (const key of baselines.keys())
      if (!queued.has(key)) baselines.delete(key);
    for (const key of checks.keys()) if (!queued.has(key)) checks.delete(key);
    for (const send of open)
      if (send.mode === 'steer' && !baselines.has(send.key))
        baselines.set(send.key, new Set(runs.map((item) => item.run.id)));

    for (const send of open) {
      if (!send.steeringRunId) continue;
      const baseline = baselines.get(send.key) ?? NO_RUNS;
      const inHistory = messages.some(
        (message) => message.metadata.clientRequestId === send.key,
      );
      const ownRun = runs.some(
        (item) =>
          item.run.id !== send.steeringRunId &&
          !baseline.has(item.run.id) &&
          item.run.input.text === send.text,
      );
      if (inHistory || ownRun) {
        settle(send.key);
        continue;
      }
      const joined = runs.find((item) => item.run.id === send.steeringRunId);
      if (!joined || !endedBadly(joined.run.status)) continue;
      const check = checks.get(send.key);
      if (check === undefined) {
        // Its message may be committed with the failed reply: read history
        // again, with a read that starts now, before deciding.
        checks.set(send.key, 'reading');
        const key = send.key;
        void refreshMessages().then(() => {
          if (!mountedRef.current || checks.get(key) !== 'reading') return;
          checks.set(key, 'read');
          setChecksLanded((value) => value + 1);
        });
      } else if (check === 'read') {
        checks.set(send.key, 'recovered');
        settle(send.key);
        onRecoverRef.current(send);
      }
    }
  }, [open, sends, messages, runs, checksLanded, settle, refreshMessages]);

  return useMemo(() => {
    const shown = shownSteers(open, runs);
    return open
      .filter((item) => !shown.has(item.key))
      .map(
        (item): PendingBubble => ({
          key: item.key,
          text: item.text,
          createdAtMs: item.createdAtMs,
          status: item.steeringRunId
            ? 'steering'
            : item.failures > 0
              ? 'retrying'
              : 'sending',
        }),
      );
  }, [open, runs]);
}
