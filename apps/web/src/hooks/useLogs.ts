import { useCallback, useEffect, useRef, useState } from 'react';
import type { LogLevel, LogLine } from '@animaOS-SWARM/sdk';

import { daemon } from '../lib/daemon-api';
import {
  LOGS_FETCH_LIMIT,
  logsErrorMessage,
  mergeLines,
  newestSeq,
  nextReconnectDelay,
} from '../lib/logs';

export interface LogsOptions {
  /** False while the daemon is offline: nothing is read or opened. */
  enabled: boolean;
  level: LogLevel | null;
  query: string;
  /** While true, new lines are held and `lines` does not change. */
  paused: boolean;
  /** Milliseconds to wait before reopening the stream after `attempt`
   *  failures in a row; tests inject 0. */
  reconnectDelay?: (attempt: number) => number;
}

export interface LogsView {
  /** Oldest first, at most `MAX_LOGS_SHOWN`. */
  lines: LogLine[];
  /** Lines that arrived while paused and are not shown yet. */
  held: number;
  /** True while the stream is open. */
  connected: boolean;
  /** The tail has been read (or failed to read). */
  loaded: boolean;
  error: string | null;
  /** The HTTP status of the failure, when the daemon refused. */
  errorStatus: number | null;
  /** Reads the tail again and reopens the stream. */
  refresh: () => void;
}

const NO_LINES: LogLine[] = [];
/** A resync larger than this many pages stops catching up. */
const MAX_CATCH_UP_PAGES = 10;

const defaultDelay = (attempt: number) =>
  nextReconnectDelay(attempt, Math.random());

function pause(ms: number, signal: AbortSignal): Promise<void> {
  if (ms <= 0 || signal.aborted) return Promise.resolve();
  return new Promise((resolve) => {
    const finish = () => {
      clearTimeout(timer);
      signal.removeEventListener('abort', finish);
      resolve();
    };
    const timer = setTimeout(finish, ms);
    signal.addEventListener('abort', finish);
  });
}

/** The Logs page's data (spec §11.2, §15.4): the recent tail, then the live
 *  stream, kept to `MAX_LOGS_SHOWN` lines. The stream closes on unmount, on
 *  a filter change, and when `enabled` turns false. */
export function useLogs({
  enabled,
  level,
  query,
  paused,
  reconnectDelay = defaultDelay,
}: LogsOptions): LogsView {
  const [lines, setLines] = useState<LogLine[]>(NO_LINES);
  const [held, setHeld] = useState(0);
  const [connected, setConnected] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [failure, setFailure] = useState<{
    message: string;
    status: number | null;
  } | null>(null);
  const [reloadKey, setReloadKey] = useState(0);

  // The refs are the source of truth, so a burst of lines merges once each.
  const shown = useRef<LogLine[]>(NO_LINES);
  const heldLines = useRef<LogLine[]>(NO_LINES);
  const pausedRef = useRef(paused);
  pausedRef.current = paused;
  const delayRef = useRef(reconnectDelay);
  delayRef.current = reconnectDelay;

  const show = useCallback((next: LogLine[]) => {
    if (next === shown.current) return;
    shown.current = next;
    setLines(next);
  }, []);

  const accept = useCallback(
    (incoming: readonly LogLine[]) => {
      if (pausedRef.current) {
        const newest = newestSeq(shown.current);
        const fresh = incoming.filter((line) => line.seq > newest);
        const next = mergeLines(heldLines.current, fresh);
        if (next !== heldLines.current) {
          heldLines.current = next;
          setHeld(next.length);
        }
        return;
      }
      show(mergeLines(shown.current, incoming));
    },
    [show],
  );

  // Resuming shows what was held.
  useEffect(() => {
    if (paused || heldLines.current.length === 0) return;
    const pending = heldLines.current;
    heldLines.current = NO_LINES;
    setHeld(0);
    show(mergeLines(shown.current, pending));
  }, [paused, show]);

  useEffect(() => {
    heldLines.current = NO_LINES;
    setHeld(0);
    show(NO_LINES);
    setConnected(false);
    setLoaded(false);
    setFailure(null);
    if (!enabled) return;

    const controller = new AbortController();
    const { signal } = controller;
    const filter = {
      level: level ?? undefined,
      q: query === '' ? undefined : query,
    };
    const live = () => !signal.aborted;

    const run = async () => {
      let attempt = 0;
      let after: number | null = null;

      while (live()) {
        try {
          if (after === null) {
            const page = await daemon.logs({
              ...filter,
              limit: LOGS_FETCH_LIMIT,
            });
            if (!live()) return;
            show(page.lines.length === 0 ? NO_LINES : page.lines);
            after = Math.max(page.newestSeq, newestSeq(page.lines));
            setFailure(null);
            setLoaded(true);
          }

          let received = false;
          for await (const event of daemon.logStream({
            ...filter,
            after,
            signal,
          })) {
            if (!live()) return;
            if (!received) {
              received = true;
              attempt = 0;
              // Only a stream that has delivered is connected, and only
              // then is an earlier failure stale.
              setConnected(true);
              setFailure(null);
            }
            if (event.kind === 'line') {
              after = Math.max(after, event.line.seq);
              accept([event.line]);
              continue;
            }
            // The stream fell behind: read what it skipped, page by page.
            for (let pages = 0; pages < MAX_CATCH_UP_PAGES; pages += 1) {
              const page = await daemon.logs({
                ...filter,
                after: Math.max(after, newestSeq(shown.current)),
                limit: LOGS_FETCH_LIMIT,
              });
              if (!live()) return;
              accept(page.lines);
              const full = page.lines.length >= LOGS_FETCH_LIMIT;
              // A full page may have more behind it: stop at its newest line
              // so the next page starts there, not past the lines between.
              after = full
                ? Math.max(after, newestSeq(page.lines))
                : Math.max(after, page.newestSeq, newestSeq(page.lines));
              if (!full) break;
            }
          }
        } catch (error) {
          if (!live()) return;
          const failed = logsErrorMessage(error);
          // A dropped connection shows only the Reconnecting note; the
          // daemon refusing (too many streams, too old) is shown.
          setFailure(failed.status === null && after !== null ? null : failed);
          setLoaded(true);
          if (failed.status === 404) {
            setConnected(false);
            return;
          }
        }
        setConnected(false);
        await pause(delayRef.current(attempt), signal);
        attempt += 1;
      }
    };
    void run();

    return () => controller.abort();
  }, [enabled, level, query, reloadKey, show, accept]);

  const refresh = useCallback(() => setReloadKey((key) => key + 1), []);

  return {
    lines,
    held,
    connected,
    loaded,
    error: failure?.message ?? null,
    errorStatus: failure?.status ?? null,
    refresh,
  };
}
