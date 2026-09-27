import { useEffect, useMemo, useState } from 'react';
import type { Run } from '@animaOS-SWARM/sdk';

import { daemon } from '../lib/daemon-api';

/** Ledger runs a session view reads for outcomes its stream no longer
 *  carries: failures, stops, and restarts (spec §4.1, §15.2). */
export const SESSION_RUNS_LIMIT = 20;

const NOTHING_LANDED: SessionLedger = { runs: [], landed: null };

function httpStatus(error: unknown): unknown {
  return typeof error === 'object' && error !== null && 'status' in error
    ? error.status
    : undefined;
}

export interface SessionLedger {
  /** The session's recent runs, newest first. */
  runs: Run[];
  /** The `refreshKey` the runs on screen were read for; null until the
   *  session's first read lands. */
  landed: number | null;
}

/** The session's ledger runs and which refresh they were read for; read
 *  again when `refreshKey` changes. A failed read keeps what the view
 *  shows, and the newest read wins. */
export function useSessionLedger(
  agentId: string | null,
  sessionId: string | null,
  refreshKey = 0,
): SessionLedger {
  const key = agentId && sessionId ? `${agentId}\u0000${sessionId}` : null;
  const [loaded, setLoaded] = useState<
    ({ key: string } & SessionLedger) | null
  >(null);
  useEffect(() => {
    if (!key || !agentId || !sessionId) return;
    let current = true;
    daemon.sessionRuns(agentId, sessionId, { limit: SESSION_RUNS_LIMIT }).then(
      (runs) => {
        if (current) setLoaded({ key, runs, landed: refreshKey });
      },
      (caught) => {
        // A daemon without the route, or a deleted session, has none.
        if (current && httpStatus(caught) === 404)
          setLoaded({ key, runs: [], landed: refreshKey });
      },
    );
    return () => {
      current = false;
    };
  }, [key, agentId, sessionId, refreshKey]);
  return useMemo(
    () =>
      loaded && loaded.key === key
        ? { runs: loaded.runs, landed: loaded.landed }
        : NOTHING_LANDED,
    [loaded, key],
  );
}

/** The session's recent runs, newest first; read again when `refreshKey`
 *  changes. A failed read keeps what the view shows. */
export function useSessionRuns(
  agentId: string | null,
  sessionId: string | null,
  refreshKey = 0,
): Run[] {
  return useSessionLedger(agentId, sessionId, refreshKey).runs;
}
