import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import type { Session } from '@animaOS-SWARM/sdk';

import { daemon } from '../lib/daemon-api';
import { sessionKey } from '../lib/session-groups';

/** The sidebar re-reads its sessions this often (the agent poll uses 5 s). */
export const SESSION_LIST_POLL_MS = 10_000;
/** Sessions loaded per page; `loadMore` reaches older ones. */
export const SESSION_LIST_LIMIT = 200;

export interface CompanionSessionFilters {
  archived: boolean;
  query: string;
}

/** The SDK's `DaemonTooOldError`, by its stable code: the daemon has no
 *  sessions routes yet (spec §13.4). */
function isDaemonTooOld(error: unknown): boolean {
  return (
    typeof error === 'object' &&
    error !== null &&
    'code' in error &&
    error.code === 'daemon_too_old'
  );
}

/**
 * A fresh first page, replacing any earlier first page in `current` (keyed
 * by `sessionKey`), while keeping every older page `loadMore` already
 * appended. A session `previousFirstPageKeys` remembers as page 1 but that
 * is absent from the fresh `firstPage` has left the window (or was deleted)
 * and is dropped; a session unique to an older page is left untouched.
 */
function mergeFirstPage(
  current: readonly Session[],
  firstPage: readonly Session[],
  previousFirstPageKeys: ReadonlySet<string>,
): Session[] {
  const freshKeys = new Set(firstPage.map(sessionKey));
  const rest = current.filter((item) => {
    const key = sessionKey(item);
    if (freshKeys.has(key)) return false; // superseded by the fresh copy below
    return !previousFirstPageKeys.has(key); // gone from page 1: drop it
  });
  return [...firstPage, ...rest];
}

/** The companion's sessions plus its helpers' (spec §3.3 `includeHelpers`). */
export function useCompanionSessions(
  agentId: string | null,
  filters: CompanionSessionFilters,
) {
  const [sessions, setSessions] = useState<Session[]>([]);
  const [loading, setLoading] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [daemonTooOld, setDaemonTooOld] = useState(false);
  const [nextCursor, setNextCursor] = useState<string | null>(null);
  const generation = useRef(0);
  const firstPageKeysRef = useRef<ReadonlySet<string>>(new Set());
  // Mirrors `daemonTooOld` for the poll scheduler below: a ref reads the
  // just-set value synchronously, before this render (and its dependent
  // effects) has a chance to commit (D3).
  const daemonTooOldRef = useRef(false);
  const query = filters.query.trim();
  const { archived } = filters;

  useLayoutEffect(() => {
    generation.current += 1;
    firstPageKeysRef.current = new Set();
    daemonTooOldRef.current = false;
    setSessions([]);
    setError(null);
    setDaemonTooOld(false);
    setNextCursor(null);
    // A new agent or filter starts a fresh paged listing: an older page
    // loaded under the previous agent or filters is no longer meaningful.
  }, [agentId, archived, query]);

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
      setSessions((current) =>
        mergeFirstPage(current, page.sessions, firstPageKeysRef.current),
      );
      firstPageKeysRef.current = new Set(page.sessions.map(sessionKey));
      setNextCursor(page.nextCursor);
      setError(null);
      setDaemonTooOld(false);
      daemonTooOldRef.current = false;
    } catch (caught) {
      if (request !== generation.current) return;
      setError(caught instanceof Error ? caught.message : String(caught));
      // Only a successful list clears it; a dropped connection proves nothing.
      if (isDaemonTooOld(caught)) {
        setDaemonTooOld(true);
        daemonTooOldRef.current = true;
      }
    } finally {
      if (request === generation.current) setLoading(false);
    }
  }, [agentId, archived, query]);

  const loadMore = useCallback(async () => {
    if (!agentId || !nextCursor) return;
    const request = ++generation.current;
    setLoadingMore(true);
    try {
      const page = await daemon.listSessions(agentId, {
        includeHelpers: true,
        archived,
        limit: SESSION_LIST_LIMIT,
        cursor: nextCursor,
        ...(query ? { q: query } : {}),
      });
      if (request !== generation.current) return;
      setSessions((current) => {
        const known = new Set(current.map(sessionKey));
        const additions = page.sessions.filter(
          (item) => !known.has(sessionKey(item)),
        );
        return [...current, ...additions];
      });
      setNextCursor(page.nextCursor);
      setError(null);
    } catch (caught) {
      if (request !== generation.current) return;
      setError(caught instanceof Error ? caught.message : String(caught));
      if (isDaemonTooOld(caught)) {
        setDaemonTooOld(true);
        daemonTooOldRef.current = true;
      }
    } finally {
      if (request === generation.current) setLoadingMore(false);
    }
  }, [agentId, archived, query, nextCursor]);

  useEffect(() => {
    if (!agentId) return;
    let active = true;
    let timer: number | undefined;
    const schedule = () => {
      // D3: once the daemon is flagged too old, stop polling rather than
      // hammering its (missing) sessions routes every 10 s; `refresh` (and
      // so `daemonTooOldRef`) still work for a manual retry.
      if (!active || daemonTooOldRef.current) return;
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

  return {
    sessions,
    loading,
    loadingMore,
    hasMore: nextCursor !== null,
    error,
    daemonTooOld,
    refresh,
    loadMore,
    upsert,
    remove,
  };
}
