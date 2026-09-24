import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import type { Session } from '@animaOS-SWARM/sdk';

import { daemon } from '../lib/daemon-api';
import { sessionKey } from '../lib/session-groups';

/** The sidebar re-reads its sessions this often (the agent poll uses 5 s). */
export const SESSION_LIST_POLL_MS = 10_000;
/** Sessions loaded at once; search reaches older ones. */
export const SESSION_LIST_LIMIT = 200;

export interface CompanionSessionFilters {
  archived: boolean;
  query: string;
}

/** The companion's sessions plus its helpers' (spec §3.3 `includeHelpers`). */
export function useCompanionSessions(
  agentId: string | null,
  filters: CompanionSessionFilters,
) {
  const [sessions, setSessions] = useState<Session[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const generation = useRef(0);
  const query = filters.query.trim();
  const { archived } = filters;

  useLayoutEffect(() => {
    generation.current += 1;
    setSessions([]);
    setError(null);
  }, [agentId]);

  const refresh = useCallback(async () => {
    if (!agentId) return;
    const request = ++generation.current;
    setLoading(true);
    try {
      const page = await daemon.listSessions(agentId, {
        includeHelpers: true,
        archived,
        limit: SESSION_LIST_LIMIT,
        ...(query ? { q: query } : {}),
      });
      if (request !== generation.current) return;
      setSessions(page.sessions);
      setError(null);
    } catch (caught) {
      if (request !== generation.current) return;
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      if (request === generation.current) setLoading(false);
    }
  }, [agentId, archived, query]);

  useEffect(() => {
    if (!agentId) return;
    let active = true;
    let timer: number | undefined;
    const schedule = () => {
      if (!active) return;
      timer = window.setTimeout(() => {
        timer = undefined;
        void refresh().finally(schedule);
      }, SESSION_LIST_POLL_MS);
    };
    void refresh().finally(schedule);
    return () => {
      active = false;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [agentId, refresh]);

  const upsert = useCallback((session: Session) => {
    setSessions((current) => [
      session,
      ...current.filter((item) => sessionKey(item) !== sessionKey(session)),
    ]);
  }, []);

  const remove = useCallback((session: Pick<Session, 'agentId' | 'id'>) => {
    setSessions((current) =>
      current.filter((item) => sessionKey(item) !== sessionKey(session)),
    );
  }, []);

  return { sessions, loading, error, refresh, upsert, remove };
}
