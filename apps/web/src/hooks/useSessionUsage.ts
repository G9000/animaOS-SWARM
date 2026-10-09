import { useEffect, useState } from 'react';
import type { UsageTotals } from '@animaOS-SWARM/sdk';

import { daemon } from '../lib/daemon-api';

/** A session's usage totals, read with the session record. The last value
 *  stays while the next read is in flight or fails (the header shows no
 *  error), but never carries over to another session. */
export function useSessionUsage(
  target: { agentId: string; sessionId: string } | null,
  refreshKey: number,
): UsageTotals | null {
  const agentId = target?.agentId ?? null;
  const sessionId = target?.sessionId ?? null;
  const key = agentId && sessionId ? `${agentId}\n${sessionId}` : null;
  const [read, setRead] = useState<{
    key: string;
    usage: UsageTotals | null;
  } | null>(null);

  useEffect(() => {
    if (!agentId || !sessionId || !key) return;
    let current = true;
    daemon
      .getSession(agentId, sessionId)
      .then((session) => {
        if (!current) return;
        const usage = session.usage ?? null;
        // Nothing new keeps the state, so a quiet header does not render.
        setRead((previous) =>
          previous
            ? previous.key === key &&
              JSON.stringify(previous.usage) === JSON.stringify(usage)
              ? previous
              : { key, usage }
            : usage === null
              ? previous
              : { key, usage },
        );
      })
      .catch(() => {
        // The header simply keeps what it had.
      });
    return () => {
      current = false;
    };
  }, [agentId, sessionId, key, refreshKey]);

  return read && read.key === key ? read.usage : null;
}
