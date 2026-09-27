import { useEffect, useState } from 'react';
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

/** The session's recent runs, newest first; read again when `refreshKey`
 *  changes. A failed read keeps what the view shows. */
export function useSessionRuns(
  agentId: string | null,
  sessionId: string | null,
  refreshKey = 0,
): Run[] {
  const key = agentId && sessionId ? `${agentId}\u0000${sessionId}` : null;
  const [loaded, setLoaded] = useState<{ key: string; runs: Run[] } | null>(
    null,
  );
  useEffect(() => {
    if (!key || !agentId || !sessionId) return;
    let current = true;
    daemon.sessionRuns(agentId, sessionId, { limit: SESSION_RUNS_LIMIT }).then(
      (runs) => {
        if (current) setLoaded({ key, runs });
      },
      (caught) => {
        // A daemon without the route, or a deleted session, has none.
        if (current && httpStatus(caught) === 404) setLoaded({ key, runs: [] });
      },
    );
    return () => {
      current = false;
    };
  }, [key, agentId, sessionId, refreshKey]);
  return loaded && loaded.key === key ? loaded.runs : NO_RUNS;
}
