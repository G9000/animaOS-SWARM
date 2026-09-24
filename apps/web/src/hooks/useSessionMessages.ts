import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import type { SessionMessage } from '@animaOS-SWARM/sdk';

import { daemon } from '../lib/daemon-api';

/** Messages per page (the daemon default). */
export const SESSION_MESSAGE_PAGE = 50;
/** The open session re-reads its newest page this often until M3's stream. */
export const SESSION_MESSAGES_POLL_MS = 3_000;

/**
 * The newest page always replaces the tail from its first message onward.
 * When that message is not found in `current` at all (a gap — the hot tail
 * rolled past messages this client never saw, from a long disconnect or a
 * burst of more than a page between polls), there is no reliable splice
 * point, so the newest page replaces the whole list outright instead of
 * guessing which older messages are still contiguous with it. Splicing by
 * comparing `createdAtMs` (the earlier approach) could re-admit a skipped
 * range out of order once `loadOlder` later fetched it, and could drop a
 * message that shared its exact millisecond with the page's first message
 * (controller ruling on ruling 2, M2 pre-flight audit fix round 1).
 */
export function mergeNewest(
  current: readonly SessionMessage[],
  page: readonly SessionMessage[],
): SessionMessage[] {
  if (page.length === 0) return [];
  const start = current.findIndex((message) => message.id === page[0].id);
  if (start < 0) return [...page];
  return [...current.slice(0, start), ...page];
}

/**
 * True when `page`'s first message is not already among `current` (and
 * `current` is non-empty) — the hot tail rolled past messages this client
 * never saw. Ruling 2 (M2 pre-flight audit): a poll that opens such a gap
 * must reset `nextBefore` from the fresh page — `mergeNewest` also replaces
 * `current` outright in this case (see above) — or scrolling back could
 * never reach the gap because `hasOlder` stayed frozen at `false`.
 */
function opensGap(
  current: readonly SessionMessage[],
  page: readonly SessionMessage[],
): boolean {
  return (
    current.length > 0 &&
    page.length > 0 &&
    !current.some((message) => message.id === page[0].id)
  );
}

function httpStatus(error: unknown): unknown {
  return typeof error === 'object' && error !== null && 'status' in error
    ? error.status
    : undefined;
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** One session's messages: the newest page, older pages on demand, and a poll. */
export function useSessionMessages(
  agentId: string | null,
  sessionId: string | null,
  refreshKey = 0,
) {
  const [messages, setMessages] = useState<SessionMessage[]>([]);
  const [nextBefore, setNextBefore] = useState<string | null>(null);
  const [loadingOlder, setLoadingOlder] = useState(false);
  const [missing, setMissing] = useState(false);
  // A newest-page error clears with the next good poll; an older-page error
  // stays until older messages load or the session changes, so a failed
  // Load older is not hidden by the poll that follows it.
  const [newestError, setNewestError] = useState<string | null>(null);
  const [olderError, setOlderError] = useState<string | null>(null);
  // Invalidates every in-flight request (both `refresh` and `loadOlder`)
  // when the hook resets for a different agent/session.
  const generation = useRef(0);
  // Invalidates only stale `refresh` calls, so an overlapping `refresh` (a
  // poll racing a `refreshKey`-triggered reload) can't apply a response that
  // is older than one already applied — without touching `loadOlder`, which
  // must keep running even while a `refresh` is in flight (Important 2,
  // fix round 1: `refresh` had no reentrancy guard of its own before this).
  const refreshGeneration = useRef(0);
  const loadedOlder = useRef(false);
  const loadingOlderRef = useRef(false);
  const missingRef = useRef(false);
  // Mirrors `messages`, updated in lockstep so a poll can synchronously
  // check the messages actually loaded so far (see `opensGap`) without
  // waiting on React to commit the corresponding state update.
  const messagesRef = useRef<SessionMessage[]>([]);

  useLayoutEffect(() => {
    generation.current += 1;
    loadedOlder.current = false;
    loadingOlderRef.current = false;
    missingRef.current = false;
    messagesRef.current = [];
    setMessages([]);
    setNextBefore(null);
    setLoadingOlder(false);
    setMissing(false);
    setNewestError(null);
    setOlderError(null);
  }, [agentId, sessionId]);

  const refresh = useCallback(async () => {
    if (!agentId || !sessionId) return;
    const sessionEpoch = generation.current;
    const request = ++refreshGeneration.current;
    try {
      const page = await daemon.sessionMessages(agentId, sessionId, {
        limit: SESSION_MESSAGE_PAGE,
      });
      if (
        sessionEpoch !== generation.current ||
        request !== refreshGeneration.current
      )
        return;
      const previous = messagesRef.current;
      const merged = mergeNewest(previous, page.messages);
      messagesRef.current = merged;
      setMessages(merged);
      // Once older history has been loaded, a normal poll must not move
      // `nextBefore` (that would discard the client's paging progress). A
      // gapped poll is the exception (ruling 2): it must reopen `hasOlder`.
      if (!loadedOlder.current || opensGap(previous, page.messages)) {
        setNextBefore(page.nextBefore);
      }
      missingRef.current = false;
      setMissing(false);
      setNewestError(null);
    } catch (caught) {
      if (
        sessionEpoch !== generation.current ||
        request !== refreshGeneration.current
      )
        return;
      if (httpStatus(caught) === 404) {
        missingRef.current = true;
        setMissing(true);
      } else {
        setNewestError(errorText(caught));
      }
    }
  }, [agentId, sessionId]);

  const loadOlder = useCallback(async () => {
    if (!agentId || !sessionId || !nextBefore || loadingOlderRef.current) return;
    const request = generation.current;
    // The id of the message this fetch's `before` cursor was derived from —
    // i.e. the list's current head. A gap-triggered `refresh` can replace
    // the whole list while this fetch is outstanding (fix round 2); if the
    // head has moved by the time it resolves, the fetched page no longer
    // attaches to anything and must be dropped, not merged onto the wrong
    // list. A non-gap `refresh` never moves the head (it only ever extends
    // the tail), so this only ever blocks a genuinely stale fetch.
    const anchor = messagesRef.current[0]?.id;
    loadingOlderRef.current = true;
    setLoadingOlder(true);
    try {
      const page = await daemon.sessionMessages(agentId, sessionId, {
        before: nextBefore,
        limit: SESSION_MESSAGE_PAGE,
      });
      if (request !== generation.current) return;
      if (messagesRef.current[0]?.id !== anchor) return;
      loadedOlder.current = true;
      const known = new Set(messagesRef.current.map((message) => message.id));
      const merged = [
        ...page.messages.filter((message) => !known.has(message.id)),
        ...messagesRef.current,
      ];
      messagesRef.current = merged;
      setMessages(merged);
      setNextBefore(page.nextBefore);
      setOlderError(null);
    } catch (caught) {
      if (request === generation.current) setOlderError(errorText(caught));
    } finally {
      if (request === generation.current) {
        loadingOlderRef.current = false;
        setLoadingOlder(false);
      }
    }
  }, [agentId, sessionId, nextBefore]);

  useEffect(() => {
    if (!agentId || !sessionId) return;
    let active = true;
    let timer: number | undefined;
    const schedule = () => {
      if (!active || missingRef.current) return;
      timer = window.setTimeout(() => {
        timer = undefined;
        void refresh().finally(schedule);
      }, SESSION_MESSAGES_POLL_MS);
    };
    void refresh().finally(schedule);
    return () => {
      active = false;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [agentId, sessionId, refresh, refreshKey]);

  return {
    messages,
    hasOlder: nextBefore !== null,
    loadingOlder,
    loadOlder,
    missing,
    error: newestError ?? olderError,
    refresh,
  };
}
