import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { Run } from '@animaOS-SWARM/sdk';

import { daemon } from '../lib/daemon-api';

/** Ledger runs a session view reads for outcomes its stream no longer
 *  carries: failures, stops, and restarts (spec §4.1, §15.2). */
export const SESSION_RUNS_LIMIT = 20;

const NO_RUNS: Run[] = [];

function httpStatus(error: unknown): unknown {
  return typeof error === 'object' && error !== null && 'status' in error
    ? error.status
    : undefined;
}

export interface SessionLedger {
  /** The session's recent runs, newest first. */
  runs: Run[];
  /** The number (in `started` order) of the read on screen; null until
   *  this session's first read lands. */
  landed: number | null;
  /** Reads begun so far, for any session: a read numbered above this
   *  began after now. */
  started: () => number;
}

/** The session's ledger runs and which read they came from; read again
 *  when `refreshKey` or the session changes. A failed read keeps what the
 *  view shows, and the newest read wins. */
export function useSessionLedger(
  agentId: string | null,
  sessionId: string | null,
  refreshKey = 0,
): SessionLedger {
  const key = agentId && sessionId ? `${agentId}\u0000${sessionId}` : null;
  const [loaded, setLoaded] = useState<{
    key: string;
    runs: Run[];
    landed: number;
  } | null>(null);
  const readsRef = useRef(0);
  const started = useCallback(() => readsRef.current, []);
  useEffect(() => {
    if (!key || !agentId || !sessionId) return;
    let current = true;
    const read = ++readsRef.current;
    // A read the view no longer wants (another session opened, a newer
    // read began, or the view closed) is aborted, not just ignored (T20).
    const controller = new AbortController();
    daemon
      .sessionRuns(agentId, sessionId, {
        limit: SESSION_RUNS_LIMIT,
        signal: controller.signal,
      })
      .then(
        (runs) => {
          if (current) setLoaded({ key, runs, landed: read });
        },
        (caught) => {
          // A daemon without the route, or a deleted session, has none.
          if (current && httpStatus(caught) === 404)
            setLoaded({ key, runs: [], landed: read });
        },
      );
    return () => {
      current = false;
      controller.abort();
    };
  }, [key, agentId, sessionId, refreshKey]);
  return useMemo(
    () =>
      loaded && loaded.key === key
        ? { runs: loaded.runs, landed: loaded.landed, started }
        : { runs: NO_RUNS, landed: null, started },
    [loaded, key, started],
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
