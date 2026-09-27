import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  isRunLifecycleEvent,
  isTerminalRunStatus,
  type AgentEvent,
  type Run,
  type SessionMessage,
} from '@animaOS-SWARM/sdk';

import {
  MAX_FINISHED_LIVE_RUNS,
  isActiveRun,
  sessionLiveRuns,
  type LiveRun,
  type LiveState,
} from '../lib/session-events';
import { mergeSessionRuns } from '../lib/transcript';
import { useAgentEvents, type AgentStreamStatus } from './useAgentEvents';
import { useSessionRuns } from './useSessionRuns';

/** Live events of one kind settle this long before what they change is read. */
export const LIVE_REFRESH_DELAY_MS = 150;

type Refresh = 'sessions' | 'messages' | 'runs';
/** The refreshes that belong to the open session, not the whole sidebar. */
const SESSION_REFRESHES: readonly Refresh[] = ['messages', 'runs'];

export interface LiveSessionTarget {
  agentId: string;
  sessionId: string;
}

export interface LiveSessionOptions {
  /** The companion whose event stream the page reads (one connection). */
  agentId: string | null;
  /** The open session (a helper's too), or null for a new chat. */
  session: LiveSessionTarget | null;
  /** Who a finished reply in the open session is credited to. */
  replyName: string;
  /** The sidebar listing's active runs for the open session, if listed. */
  listedActiveRuns: number | null;
  /** The open session's loaded history. */
  messages: readonly SessionMessage[];
  /** Runs whose end the view waits for, such as a Telegram reply's. */
  watchedRunIds?: readonly string[];
  refreshSessions: () => void;
  refreshMessages: () => void;
}

export interface LiveSessionView {
  status: AgentStreamStatus;
  /** Every live run of the companion and its helpers. */
  state: LiveState;
  /** The open session's runs, oldest first: the stream's view of each,
   *  then the ledger's record for the rest. */
  runs: LiveRun[];
  /** The open session's reply in progress. */
  activeRun: Run | null;
  /** The newest finished reply, for a polite live region (spec §15.5). */
  announcement: string;
  /** Reads the open session's ledger again. */
  refreshRuns: () => void;
}

const NO_WATCHED: readonly string[] = [];

function clearTimers(timers: Map<Refresh, number>, kinds: readonly Refresh[]) {
  for (const kind of kinds) {
    const timer = timers.get(kind);
    if (timer === undefined) continue;
    window.clearTimeout(timer);
    timers.delete(kind);
  }
}

/**
 * The companion's event stream as the open session uses it (spec §6,
 * §15.5): session events refresh the sidebar, message events the open
 * session, and lifecycle events its ledger runs, each after a 150 ms
 * settle; a snapshot or resync refreshes all three. Without the stream
 * the ledger is read again when the listing's count of active runs moves,
 * and, once per run, when history shows a run the view thinks is still
 * going (or one it waits for).
 */
export function useLiveSession({
  agentId,
  session,
  replyName,
  listedActiveRuns,
  messages,
  watchedRunIds = NO_WATCHED,
  refreshSessions,
  refreshMessages,
}: LiveSessionOptions): LiveSessionView {
  const sessionAgentId = session?.agentId ?? null;
  const sessionId = session?.sessionId ?? null;
  const key =
    sessionAgentId && sessionId ? `${sessionAgentId}\u0000${sessionId}` : null;
  const [runsRefresh, setRunsRefresh] = useState(0);
  const refreshRuns = useCallback(
    () => setRunsRefresh((value) => value + 1),
    [],
  );
  const ledger = useSessionRuns(sessionAgentId, sessionId, runsRefresh);

  const timersRef = useRef(new Map<Refresh, number>());
  // A companion's refreshes end with it (and with the view); the open
  // session's end when another opens, which reads its own on open.
  useEffect(() => {
    const timers = timersRef.current;
    return () => clearTimers(timers, ['sessions', ...SESSION_REFRESHES]);
  }, [agentId]);
  useEffect(() => {
    const timers = timersRef.current;
    return () => clearTimers(timers, SESSION_REFRESHES);
  }, [key]);
  /** Runs `refresh` once events of one kind settle. */
  const refreshSoon = (kind: Refresh, refresh: () => void) => {
    clearTimers(timersRef.current, [kind]);
    timersRef.current.set(
      kind,
      window.setTimeout(() => {
        timersRef.current.delete(kind);
        refresh();
      }, LIVE_REFRESH_DELAY_MS),
    );
  };

  const [announced, setAnnounced] = useState<{
    key: string;
    text: string;
  } | null>(null);
  /** Finished replies already announced, so a repeat is not read again. */
  const announcedRunsRef = useRef(new Set<string>());
  const announce = (openKey: string, runId: string) => {
    const done = announcedRunsRef.current;
    if (done.has(runId)) return;
    done.add(runId);
    if (done.size > MAX_FINISHED_LIVE_RUNS)
      done.delete(done.values().next().value as string);
    const text = `${replyName} replied.`;
    // A second reply differs by a trailing space so it is read again.
    setAnnounced((current) => ({
      key: openKey,
      text:
        current?.key === openKey && current.text === text ? `${text} ` : text,
    }));
  };

  const onEvent = (event: AgentEvent) => {
    if (event.type === 'stream.snapshot' || event.type === 'stream.resync') {
      // A new stream, or one that fell behind: read again what is shown.
      refreshSoon('sessions', refreshSessions);
      refreshSoon('messages', refreshMessages);
      refreshSoon('runs', refreshRuns);
      return;
    }
    const lifecycle = isRunLifecycleEvent(event);
    const sessionChange = event.type.startsWith('session.');
    if (lifecycle || sessionChange) refreshSoon('sessions', refreshSessions);
    if (
      !key ||
      event.agentId !== sessionAgentId ||
      event.sessionId !== sessionId
    )
      return;
    // The open session's own change (a summary, or its deletion elsewhere)
    // shows through its messages too.
    if (
      event.type === 'message.created' ||
      sessionChange ||
      (lifecycle && isTerminalRunStatus(event.run.status))
    )
      refreshSoon('messages', refreshMessages);
    if (lifecycle) refreshSoon('runs', refreshRuns);
    if (event.type === 'run.completed') announce(key, event.run.id);
  };
  const live = useAgentEvents(agentId, onEvent);

  const runs = useMemo(
    () =>
      sessionAgentId && sessionId
        ? mergeSessionRuns(
            sessionLiveRuns(live.state, sessionAgentId, sessionId),
            ledger,
          )
        : [],
    [sessionAgentId, sessionId, live.state, ledger],
  );
  const activeRun = runs.find((item) => isActiveRun(item.run))?.run ?? null;

  // Without the stream nothing says a run ended: the listing's count moving
  // does (the sidebar polls).
  const listedRef = useRef<{ key: string; count: number } | null>(null);
  const streamOpen = live.status === 'open';
  useEffect(() => {
    if (!key || listedActiveRuns === null) return;
    const previous = listedRef.current;
    listedRef.current = { key, count: listedActiveRuns };
    if (
      !streamOpen &&
      previous?.key === key &&
      previous.count !== listedActiveRuns
    )
      refreshRuns();
  }, [key, listedActiveRuns, streamOpen, refreshRuns]);

  // A run's messages are committed when it ends: history showing a run the
  // view thinks is still going, or one it waits for, prompts one read of
  // the ledger. A run checked once is not read again, found or not.
  const checkedRef = useRef(new Set<string>());
  useEffect(() => {
    checkedRef.current.clear();
  }, [key]);
  useEffect(() => {
    if (!key) return;
    const finished = new Set<string>();
    const going = new Set<string>();
    for (const item of runs)
      (isTerminalRunStatus(item.run.status) ? finished : going).add(
        item.run.id,
      );
    let stale = false;
    for (const message of messages) {
      const runId = message.metadata.runId;
      if (
        typeof runId !== 'string' ||
        finished.has(runId) ||
        checkedRef.current.has(runId) ||
        !(going.has(runId) || watchedRunIds.includes(runId))
      )
        continue;
      checkedRef.current.add(runId);
      stale = true;
    }
    if (stale) refreshRuns();
  }, [key, messages, runs, watchedRunIds, refreshRuns]);

  return {
    status: live.status,
    state: live.state,
    runs,
    activeRun,
    announcement: announced && announced.key === key ? announced.text : '',
    refreshRuns,
  };
}
