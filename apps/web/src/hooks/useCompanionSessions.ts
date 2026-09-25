import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from 'react';
import type { Session } from '@animaOS-SWARM/sdk';

import { daemon } from '../lib/daemon-api';
import { sessionKey } from '../lib/session-groups';

/** The sidebar re-reads its sessions this often (the agent poll uses 5 s). */
export const SESSION_LIST_POLL_MS = 10_000;
/** Sessions per page of the daemon's listing. */
export const SESSION_LIST_LIMIT = 200;
/** Pages a refresh reads at most: `loadMore` adds one page up to this cap. */
export const SESSION_LIST_MAX_PAGES = 10;

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
 * The companion's sessions plus its helpers' (spec §3.3 `includeHelpers`).
 *
 * The list is the first `k` pages of the daemon's listing (residual round
 * R2). Every refresh (the 10 s poll, a manual `refresh()`, and the refreshes
 * after a rename, archive, or read mark) walks the cursor from page 1 through
 * page `k`, then replaces the list with what it read in one update, so a
 * session that moved between pages shows once and older pages stay current.
 * The newest walk wins; a superseded walk is discarded. `loadMore` raises `k`
 * by one, up to `SESSION_LIST_MAX_PAGES`, and walks again. A new agent or
 * filter starts over from one page but keeps the previous list on screen
 * until the new first page lands.
 */
export function useCompanionSessions(
  agentId: string | null,
  filters: CompanionSessionFilters,
) {
  const [sessions, setSessions] = useState<Session[]>([]);
  const [loading, setLoading] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [hasMore, setHasMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [daemonTooOld, setDaemonTooOld] = useState(false);
  /** Bumped by every walk and every agent or filter change: only the walk
   *  holding the current value may land. */
  const generation = useRef(0);
  /** `k`: the pages every walk reads. */
  const pagesRef = useRef(1);
  /** The pages the list on screen was read from. */
  const shownPagesRef = useRef(1);
  const hasMoreRef = useRef(false);
  /** The `k` a `loadMore` waits for: `loadingMore` holds until a walk that
   *  reads that many pages lands, or until the newest walk fails. */
  const loadMoreTargetRef = useRef<number | null>(null);
  // Mirrors `daemonTooOld` for the poll scheduler below: a ref reads the
  // just-set value synchronously, before this render (and its dependent
  // effects) has a chance to commit (D3).
  const daemonTooOldRef = useRef(false);
  /** Arms the next poll while the poll effect runs; a walk that succeeds
   *  after the daemon was flagged too old re-arms the poll through it (R3). */
  const resumePollRef = useRef<(() => void) | null>(null);
  const query = filters.query.trim();
  const { archived } = filters;

  useLayoutEffect(() => {
    // A new agent or filter starts over from one page: any walk in flight is
    // superseded, and a pending `loadMore` belonged to the old listing.
    generation.current += 1;
    pagesRef.current = 1;
    shownPagesRef.current = 1;
    loadMoreTargetRef.current = null;
    setLoadingMore(false);
    // The previous list stays on screen until the new first page lands.
    if (agentId) return;
    hasMoreRef.current = false;
    daemonTooOldRef.current = false;
    setSessions([]);
    setHasMore(false);
    setLoading(false);
    setError(null);
    setDaemonTooOld(false);
  }, [agentId, archived, query]);

  const refresh = useCallback(async () => {
    if (!agentId) return;
    const request = ++generation.current;
    const pages = pagesRef.current;
    setLoading(true);
    try {
      const listed: Session[] = [];
      const seen = new Set<string>();
      let cursor: string | null = null;
      let read = 0;
      do {
        const page = await daemon.listSessions(agentId, {
          includeHelpers: true,
          archived,
          limit: SESSION_LIST_LIMIT,
          ...(cursor ? { cursor } : {}),
          ...(query ? { q: query } : {}),
        });
        if (request !== generation.current) return; // superseded: discarded
        for (const session of page.sessions) {
          const key = sessionKey(session);
          if (seen.has(key)) continue; // the first occurrence wins
          seen.add(key);
          listed.push(session);
        }
        cursor = page.nextCursor;
        read += 1;
      } while (cursor !== null && read < pages);
      const more = cursor !== null && pages < SESSION_LIST_MAX_PAGES;
      shownPagesRef.current = pages;
      hasMoreRef.current = more;
      setSessions(listed);
      setHasMore(more);
      setError(null);
      setDaemonTooOld(false);
      if (daemonTooOldRef.current) {
        daemonTooOldRef.current = false;
        resumePollRef.current?.();
      }
      const target = loadMoreTargetRef.current;
      if (target !== null && pages >= target) {
        loadMoreTargetRef.current = null;
        setLoadingMore(false);
      }
    } catch (caught) {
      if (request !== generation.current) return;
      setError(caught instanceof Error ? caught.message : String(caught));
      // Only a successful list clears it; a dropped connection proves nothing.
      if (isDaemonTooOld(caught)) {
        setDaemonTooOld(true);
        daemonTooOldRef.current = true;
      }
      if (loadMoreTargetRef.current !== null) {
        // The page `loadMore` asked for was not read: back to the pages the
        // list on screen came from, so the next click asks for it again.
        loadMoreTargetRef.current = null;
        pagesRef.current = shownPagesRef.current;
        setLoadingMore(false);
      }
    } finally {
      if (request === generation.current) setLoading(false);
    }
  }, [agentId, archived, query]);

  const loadMore = useCallback(async () => {
    if (
      !agentId ||
      !hasMoreRef.current ||
      loadMoreTargetRef.current !== null ||
      pagesRef.current >= SESSION_LIST_MAX_PAGES
    )
      return;
    pagesRef.current += 1;
    loadMoreTargetRef.current = pagesRef.current;
    setLoadingMore(true);
    // A walk started meanwhile (a poll, a refresh) also reads the raised `k`,
    // so a newer walk superseding this one still clears `loadingMore`.
    await refresh();
  }, [agentId, refresh]);

  useEffect(() => {
    if (!agentId) return;
    let active = true;
    let timer: number | undefined;
    const schedule = () => {
      // D3: once the daemon is flagged too old, stop polling rather than
      // hammering its (missing) sessions routes every 10 s; a later walk that
      // succeeds (a manual `refresh`) re-arms the poll through
      // `resumePollRef` (R3).
      if (!active || timer !== undefined || daemonTooOldRef.current) return;
      timer = window.setTimeout(() => {
        timer = undefined;
        void refresh().finally(schedule);
      }, SESSION_LIST_POLL_MS);
    };
    resumePollRef.current = schedule;
    void refresh().finally(schedule);
    return () => {
      active = false;
      if (resumePollRef.current === schedule) resumePollRef.current = null;
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
    hasMore,
    error,
    daemonTooOld,
    refresh,
    loadMore,
    upsert,
    remove,
  };
}
