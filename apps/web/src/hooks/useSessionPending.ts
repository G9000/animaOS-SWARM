import { useEffect, useMemo, useRef } from 'react';
import type { Run, SessionMessage } from '@animaOS-SWARM/sdk';

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
  /** The open session's ledger: its runs on screen, the number of the read
   *  they came from (null before the session's first), and how many reads
   *  have begun (reads are numbered as they begin). */
  ledger: {
    landed: number | null;
    runs: readonly Run[];
    started: () => number;
  };
  /** The number of the newest history read on screen (0 before one). */
  appliedRead: number;
  /** History reads begun so far (reads are numbered as they begin). */
  readsStarted: () => number;
  /** Reads the open session's newest messages again. */
  refreshMessages: () => unknown;
  /** Reads the open session's ledger again. */
  refreshRuns: () => void;
  /** A steer that no run took and no history holds: its text goes to the
   *  composer's recovery panel. */
  onRecover: (send: SessionSend) => void;
}

/** The runs a steer cannot have become: those its session had when it was
 *  sent, and those of any ledger read begun by then (numbered up to
 *  `before`). */
interface SteerBaseline {
  ids: Set<string>;
  before: number;
}

/** A failed reply's steer, waiting for a history read and a ledger read
 *  begun after the failure. Reads are numbered as they begin. */
interface RecoveryCheck {
  /** History reads begun by the failure: a read numbered above counts. */
  historyAfter: number;
  /** Ledger reads begun by the failure: a read numbered above counts. */
  ledgerAfter: number;
  /** History reads begun when the ledger was last asked: until a ledger
   *  read lands, each applied history read numbered above asks once more. */
  ledgerAskedAt: number;
}

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
 * `failed` or `interrupted`, the text moves to the recovery panel only once
 * a history read begun after the failure was applied and a ledger read
 * begun after it landed, both without it (the daemon announces a steer's
 * own run after the failure, so a ledger read may be the first to show
 * it). Failed reads are not retried here: the poll and the stream's events
 * read again, and the ledger is asked again at most once per history read.
 */
export function useSessionPending({
  sends,
  settle,
  session,
  messages,
  runs,
  ledger,
  appliedRead,
  readsStarted,
  refreshMessages,
  refreshRuns,
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

  const baselinesRef = useRef(new Map<string, SteerBaseline>());
  const checksRef = useRef(new Map<string, RecoveryCheck>());
  /** Steers already handed to the recovery panel, until they leave the
   *  queue. */
  const recoveredRef = useRef(new Set<string>());
  const onRecoverRef = useRef(onRecover);
  useEffect(() => {
    onRecoverRef.current = onRecover;
  });
  // Another session's history and ledger say nothing about these steers:
  // a recovery starts over when their session opens again.
  useEffect(() => {
    checksRef.current.clear();
  }, [agentId, sessionId]);

  useEffect(() => {
    const baselines = baselinesRef.current;
    const checks = checksRef.current;
    const queued = new Set(sends.map((item) => item.key));
    for (const key of baselines.keys())
      if (!queued.has(key)) baselines.delete(key);
    for (const key of checks.keys()) if (!queued.has(key)) checks.delete(key);
    for (const key of recoveredRef.current)
      if (!queued.has(key)) recoveredRef.current.delete(key);
    for (const send of open) {
      if (send.mode !== 'steer') continue;
      let baseline = baselines.get(send.key);
      if (!baseline) {
        baseline = {
          ids: new Set(runs.map((item) => item.run.id)),
          before: ledger.started(),
        };
        baselines.set(send.key, baseline);
      }
      // A ledger read begun before the steer went out cannot hold its run,
      // even when it lands later.
      if (ledger.landed !== null && ledger.landed <= baseline.before)
        for (const run of ledger.runs) baseline.ids.add(run.id);
    }

    for (const send of open) {
      if (!send.steeringRunId || recoveredRef.current.has(send.key)) continue;
      const baseline = baselines.get(send.key);
      const inHistory = messages.some(
        (message) => message.metadata.clientRequestId === send.key,
      );
      const ownRun = runs.some(
        (item) =>
          item.run.id !== send.steeringRunId &&
          !baseline?.ids.has(item.run.id) &&
          item.run.input.text === send.text,
      );
      if (inHistory || ownRun) {
        checks.delete(send.key);
        settle(send.key);
        continue;
      }
      const joined = runs.find((item) => item.run.id === send.steeringRunId);
      if (!joined || !endedBadly(joined.run.status)) continue;
      const ledgerLanded = (after: number) =>
        ledger.landed !== null && ledger.landed > after;
      const check = checks.get(send.key);
      if (!check) {
        // Its message may be committed with the failed reply, or its own
        // run announced after it: ask for one read of each, then wait for
        // reads begun from now, whatever starts them. It never retries on
        // its own: the poll and the stream's events keep reading.
        const historyAfter = readsStarted();
        const ledgerAfter = ledger.started();
        refreshRuns();
        refreshMessages();
        checks.set(send.key, {
          historyAfter,
          ledgerAfter,
          ledgerAskedAt: readsStarted(),
        });
        continue;
      }
      if (appliedRead <= check.historyAfter) continue;
      if (!ledgerLanded(check.ledgerAfter)) {
        // The ledger's read failed, or was superseded: ask again, at most
        // once per history read, so at the poll's pace.
        if (appliedRead > check.ledgerAskedAt) {
          check.ledgerAskedAt = readsStarted();
          refreshRuns();
        }
        continue;
      }
      checks.delete(send.key);
      recoveredRef.current.add(send.key);
      settle(send.key);
      onRecoverRef.current(send);
    }
  }, [
    open,
    sends,
    messages,
    runs,
    ledger,
    appliedRead,
    readsStarted,
    settle,
    refreshMessages,
    refreshRuns,
  ]);

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
