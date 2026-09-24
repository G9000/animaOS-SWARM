import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import type { SessionMessage } from '@animaOS-SWARM/sdk';

import { daemon } from '../lib/daemon-api';

/** Messages per page (the daemon default). */
export const SESSION_MESSAGE_PAGE = 50;
/** The open session re-reads its newest page this often until M3's stream. */
export const SESSION_MESSAGES_POLL_MS = 3_000;

/** The newest page replaces the tail; older loaded messages stay. */
export function mergeNewest(
  current: readonly SessionMessage[],
  page: readonly SessionMessage[],
): SessionMessage[] {
  if (page.length === 0) return [];
  const start = current.findIndex((message) => message.id === page[0].id);
  const older =
    start >= 0
      ? current.slice(0, start)
      : current.filter((message) => message.createdAtMs < page[0].createdAtMs);
  return [...older, ...page];
}

/**
 * True when `page`'s first message is not already among `current` (and
 * `current` is non-empty) — the hot tail rolled past messages this client
 * never saw. Ruling 2 (M2 pre-flight audit): a poll that opens such a gap
 * must not leave `nextBefore` frozen, or the missing range becomes
 * permanently unreachable through "load older".
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
  const [error, setError] = useState<string | null>(null);
  const generation = useRef(0);
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
    setError(null);
  }, [agentId, sessionId]);

  const refresh = useCallback(async () => {
    if (!agentId || !sessionId) return;
    const request = generation.current;
    try {
      const page = await daemon.sessionMessages(agentId, sessionId, {
        limit: SESSION_MESSAGE_PAGE,
      });
      if (request !== generation.current) return;
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
      setError(null);
    } catch (caught) {
      if (request !== generation.current) return;
      if (httpStatus(caught) === 404) {
        missingRef.current = true;
        setMissing(true);
      } else {
        setError(errorText(caught));
      }
    }
  }, [agentId, sessionId]);

  const loadOlder = useCallback(async () => {
    if (!agentId || !sessionId || !nextBefore || loadingOlderRef.current) return;
    const request = generation.current;
    loadingOlderRef.current = true;
    setLoadingOlder(true);
    try {
      const page = await daemon.sessionMessages(agentId, sessionId, {
        before: nextBefore,
        limit: SESSION_MESSAGE_PAGE,
      });
      if (request !== generation.current) return;
      loadedOlder.current = true;
      const known = new Set(messagesRef.current.map((message) => message.id));
      const merged = [
        ...page.messages.filter((message) => !known.has(message.id)),
        ...messagesRef.current,
      ];
      messagesRef.current = merged;
      setMessages(merged);
      setNextBefore(page.nextBefore);
    } catch (caught) {
      if (request === generation.current) setError(errorText(caught));
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
    error,
    refresh,
  };
}
