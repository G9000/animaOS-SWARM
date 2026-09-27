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
import { useSessionLedger, type SessionLedger } from './useSessionRuns';

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
  /** The open session's runs still going, as the open stream counts them
   *  (its snapshot has every active run); null while it is not open. */
  activeRunCount: number | null;
  /** The newest finished reply, for a polite live region (spec §15.5). */
  announcement: string;
  /** The open session's ledger: its runs on screen, the number of the read
   *  they came from (null before the session's first), and how many reads
   *  have begun. */
  ledger: SessionLedger;
  /** Reads the open session's ledger again. */
  refreshRuns: () => void;
  /** Shows a run the daemon just accepted into the open session until the
   *  stream or a later ledger read has it. */
  seedRun: (run: Run) => void;
}

/** An accepted run shown before the stream or the ledger has it. */
interface SeededRun {
  key: string;
  run: Run;
  /** The ledger reads begun when it was accepted. */
  after: number;
}

const NO_WATCHED: readonly string[] = [];
const NO_SEEDS: readonly SeededRun[] = [];

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
  // Its reads are numbered as they begin, so a caller can wait for one
  // that began after something happened.
  const ledger = useSessionLedger(sessionAgentId, sessionId, runsRefresh);
  const startedLedgerReads = ledger.started;

  const [announced, setAnnounced] = useState<{
    key: string;
    text: string;
  } | null>(null);
  const timersRef = useRef(new Map<Refresh, number>());
  // A companion's refreshes end with it (and with the view); the open
  // session's end when another opens, which reads its own on open.
  useEffect(() => {
    const timers = timersRef.current;
    return () => clearTimers(timers, ['sessions', ...SESSION_REFRESHES]);
  }, [agentId]);
  useEffect(() => {
    // A reply announced in the session left behind is not read again when
    // it is reopened.
    setAnnounced(null);
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

  // Accepted runs fill the gap between the daemon's answer and the
  // stream's `run.queued` or the ledger's next read (spec §15.2).
  const [seeds, setSeeds] = useState(NO_SEEDS);
  const keyRef = useRef(key);
  keyRef.current = key;
  const seedRun = useCallback(
    (run: Run) => {
      const runKey = `${run.agentId}\u0000${run.sessionId}`;
      if (runKey !== keyRef.current) return;
      const seed = { key: runKey, run, after: startedLedgerReads() };
      setSeeds((current) => [
        ...current.filter((item) => item.run.id !== run.id),
        seed,
      ]);
    },
    [startedLedgerReads],
  );
  const seedAnswered = useCallback(
    (seed: SeededRun) =>
      seed.key !== key ||
      seed.run.id in live.state.runs ||
      (ledger.landed !== null && ledger.landed > seed.after) ||
      ledger.runs.some((run) => run.id === seed.run.id),
    [key, live.state, ledger],
  );
  useEffect(() => {
    setSeeds((current) => {
      const kept = current.filter((seed) => !seedAnswered(seed));
      return kept.length === current.length ? current : kept;
    });
  }, [seedAnswered]);

  const runs = useMemo(() => {
    if (!sessionAgentId || !sessionId) return [];
    const seeded = seeds
      .filter((seed) => !seedAnswered(seed))
      .map((seed) => seed.run);
    return mergeSessionRuns(
      sessionLiveRuns(live.state, sessionAgentId, sessionId),
      seeded.length > 0 ? [...ledger.runs, ...seeded] : ledger.runs,
    );
  }, [sessionAgentId, sessionId, live.state, ledger, seeds, seedAnswered]);
  const activeRun = runs.find((item) => isActiveRun(item.run))?.run ?? null;
  const streamOpen = live.status === 'open';
  const activeRunCount = streamOpen
    ? runs.filter((item) => !isTerminalRunStatus(item.run.status)).length
    : null;

  // Without the stream nothing says a run ended: the listing's count moving
  // does (the sidebar polls).
  const listedRef = useRef<{ key: string; count: number } | null>(null);
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
  // the ledger. A run checked once is not read again, found or not. A
  // finished run's messages committed drop its steps and tool cards from
  // live state (S3b-B): the transcript renders it from history from then
  // on, so the stream's own copy just holds memory.
  const checkedRef = useRef(new Set<string>());
  useEffect(() => {
    checkedRef.current.clear();
  }, [key]);
  const trimFinishedRun = live.trimFinishedRun;
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
      if (typeof runId !== 'string') continue;
      if (finished.has(runId)) {
        trimFinishedRun(runId);
        continue;
      }
      if (
        checkedRef.current.has(runId) ||
        !(going.has(runId) || watchedRunIds.includes(runId))
      )
        continue;
      checkedRef.current.add(runId);
      stale = true;
    }
    if (stale) refreshRuns();
  }, [key, messages, runs, watchedRunIds, refreshRuns, trimFinishedRun]);

  return {
    status: live.status,
    state: live.state,
    runs,
    activeRun,
    activeRunCount,
    announcement: announced && announced.key === key ? announced.text : '',
    ledger,
    refreshRuns,
    seedRun,
  };
}
