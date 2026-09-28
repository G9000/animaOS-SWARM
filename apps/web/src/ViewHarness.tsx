import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import {
  isTerminalRunStatus,
  type RunMode,
  type Session,
} from '@animaOS-SWARM/sdk';

import { AlertIcon } from './components/icons';
import { CompanionSetup } from './components/onboarding/CompanionSetup';
import { SettingsPanel } from './components/SettingsPanel';
import { ConnectorsView } from './components/ConnectorsView';
import { SessionSidebar } from './components/sessions/SessionSidebar';
import { SessionView } from './components/sessions/SessionView';
import { TelegramSettings } from './components/TelegramSettings';
import { WorkspaceShell, availablePage } from './components/WorkspaceShell';
import { useAgentIntegrations } from './hooks/useAgentIntegrations';
import { useCompanionSessions } from './hooks/useCompanionSessions';
import { useDaemonBootstrap } from './hooks/useDaemonBootstrap';
import { useLiveSession } from './hooks/useLiveSession';
import { useSessionCommands } from './hooks/useSessionCommands';
import { useSessionPending } from './hooks/useSessionPending';
import { useSessionSends } from './hooks/useSessionSends';
import { useTranscriptActions } from './hooks/useTranscriptActions';
import {
  SESSION_MESSAGES_LIVE_POLL_MS,
  SESSION_MESSAGES_POLL_MS,
  useSessionMessages,
} from './hooks/useSessionMessages';
import { clearCheckins, importLegacyCheckins } from './lib/checkins';
import {
  daemon,
  toAgentDetail,
  toChatMessage,
  type AgentUpdateInput,
  type DaemonSnapshot,
} from './lib/daemon-api';
import { selectMainAgent } from './lib/agent-access';
import { useHashRoute, type HashRoute } from './lib/hash-route';
import { exportFileName, sessionKey } from './lib/session-groups';
import { loadDraft, storeDraft } from './lib/drafts';
import { SLASH_COMMANDS } from './lib/slash-commands';
import { safeIntegrationError } from './lib/telegram';

interface AgentOperation {
  generation: number;
  lifecycleGeneration: number;
  targetAgentId: string;
}

interface FailedDraft {
  requestId: string;
  text: string;
  /** A failed message keeps its key, so resending it unchanged is joined, not doubled. */
  idempotencyKey?: string;
}

type ChatState = {
  draft: string;
  failedDrafts: FailedDraft[];
  /** A new chat is creating its session; its first message waits. */
  sending: boolean;
  error: string | null;
  /** A restored message; sent again unchanged, it reuses its key. */
  resend: { text: string; idempotencyKey: string } | null;
  /** The last Telegram reply's run; `queued` once it completed while the
   *  connector had an approved chat, which queues its reply for delivery. */
  delivery: { runId: string; queued: boolean } | null;
};

const EMPTY_CHAT: ChatState = {
  draft: '',
  failedDrafts: [],
  sending: false,
  error: null,
  resend: null,
  delivery: null,
};
const HOME_CONVERSATION = 'home';

function httpStatus(error: unknown): unknown {
  return typeof error === 'object' && error !== null && 'status' in error
    ? error.status
    : undefined;
}

/** Chat state is kept per agent and conversation (`home` or `session:<id>`). */
function chatKey(agentId: string, conversation: string): string {
  return `${agentId}\u0000${conversation}`;
}

function sessionConversation(sessionId: string): string {
  return `session:${sessionId}`;
}

function chatState(chats: Record<string, ChatState>, key: string): ChatState {
  return chats[key] ?? { ...EMPTY_CHAT, draft: loadDraft(key) };
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function saveTextFile(name: string, text: string) {
  const url = URL.createObjectURL(new Blob([text], { type: 'text/markdown' }));
  const link = document.createElement('a');
  link.href = url;
  link.download = name;
  link.click();
  URL.revokeObjectURL(url);
}

function ConnectingState() {
  return (
    <main
      className="relative z-[1] flex min-h-0 flex-1 items-center justify-center"
      aria-live="polite"
      aria-busy="true"
    >
      <div className="flex flex-col items-center gap-3 text-center">
        <div className="flex items-center gap-1.5" aria-hidden>
          {[0, 1, 2].map((index) => (
            <span
              key={index}
              className="typing-dot h-2 w-2 rounded-full bg-ink-3"
              style={{ animationDelay: `${index * 150}ms` }}
            />
          ))}
        </div>
        <p className="font-display text-sm font-medium text-ink">
          Connecting to anima-daemon…
        </p>
        <p className="font-mono text-[11px] text-ink-3">
          Checking daemon availability
        </p>
      </div>
    </main>
  );
}

function OfflineRetry({ retry }: { retry: () => Promise<void> }) {
  return (
    <main className="relative z-[1] flex min-h-0 flex-1 items-center justify-center px-5">
      <section
        role="alert"
        className="glass-strong w-full max-w-lg rounded-3xl p-7 text-center sm:p-9"
      >
        <div className="mx-auto flex h-12 w-12 items-center justify-center rounded-full bg-danger/10 text-danger">
          <AlertIcon size={20} />
        </div>
        <h1 className="mt-4 font-display text-2xl font-semibold tracking-tight text-ink">
          Offline
        </h1>
        <p className="mx-auto mt-2 max-w-sm text-sm leading-relaxed text-ink-2">
          The workspace cannot reach anima-daemon yet. Start the Rust host, then
          retry this connection.
        </p>
        <code className="mt-4 inline-block rounded-xl border border-line bg-abyss/60 px-3 py-2 font-mono text-xs text-mint">
          bun dev --host rust
        </code>
        <div className="mt-5">
          <button
            type="button"
            autoFocus
            onClick={() => void retry()}
            className="rounded-xl bg-accent px-4 py-2 text-sm font-semibold text-accent-fg shadow-lg shadow-accent/20 transition hover:bg-accent/90"
          >
            Retry connection
          </button>
        </div>
      </section>
    </main>
  );
}

export function ViewHarness() {
  // Once the companion's stream is open, polls slow down and events drive
  // refreshes (spec §15.5).
  const [streamOpen, setStreamOpen] = useState(false);
  const {
    connection,
    loaded,
    agents: agentSnapshots,
    providers,
    providersError,
    workspace,
    refreshAgents,
    retryProviders,
    refreshWorkspace,
    acceptAgentSnapshot,
    removeAgentSnapshot,
  } = useDaemonBootstrap({ live: streamOpen });
  const agents = useMemo(
    () => agentSnapshots.map((snapshot) => toAgentDetail(snapshot)),
    [agentSnapshots],
  );
  const mainAgent = selectMainAgent(agents);
  // Helpers are implementation details, never a second top-level persona.
  const agent = mainAgent;
  const agentId = agent?.id ?? null;
  const availableAgentIdsRef = useRef(new Set<string>());
  availableAgentIdsRef.current = new Set(agents.map((item) => item.id));
  const [route, navigate] = useHashRoute();
  const routeRef = useRef(route);
  routeRef.current = route;
  // A page hides the conversation but keeps it: the last chat or session stays loaded.
  const lastConversationRef = useRef<HashRoute>({ kind: 'home' });
  if (route.kind !== 'page') lastConversationRef.current = route;
  const conversationRoute = lastConversationRef.current;

  const [sessionQuery, setSessionQuery] = useState('');
  const [showArchived, setShowArchived] = useState(false);
  const [sessionActionError, setSessionActionError] = useState<string | null>(
    null,
  );
  const sessions = useCompanionSessions(
    agentId,
    { archived: showArchived, query: sessionQuery },
    { live: streamOpen },
  );
  // A daemon with sessions but no runs route (M2) answers a send for a
  // session that exists with 404 (spec §13.4).
  const [runsRouteMissing, setRunsRouteMissing] = useState(false);
  const listedSessionsRef = useRef(sessions.sessions);
  listedSessionsRef.current = sessions.sessions;
  const routeSessionId =
    conversationRoute.kind === 'session' ? conversationRoute.sessionId : null;
  // Another agent's session (a helper's) names its agent in the route.
  const routeAgentId =
    conversationRoute.kind === 'session'
      ? (conversationRoute.agentId ?? agentId)
      : null;
  const listedSession =
    routeSessionId && routeAgentId
      ? (sessions.sessions.find(
          (item) => item.id === routeSessionId && item.agentId === routeAgentId,
        ) ?? null)
      : null;
  const sessionListed = listedSession !== null;
  // A session outside the loaded list (archived, older, or filtered out by a
  // search) keeps its last known record and is read again on its own.
  const [knownSession, setKnownSession] = useState<Session | null>(null);
  const [sessionReadError, setSessionReadError] = useState<string | null>(null);
  useEffect(() => {
    if (listedSession) setKnownSession(listedSession);
  }, [listedSession]);
  useEffect(() => {
    setSessionReadError(null);
    if (!routeSessionId || sessionListed || !routeAgentId) return;
    let active = true;
    let timer: number | undefined;
    // A failed read is retried on the messages' cadence while the route
    // points here; a missing session shows as deleted through its messages.
    const read = () => {
      daemon.getSession(routeAgentId, routeSessionId).then(
        (session) => {
          if (!active) return;
          setKnownSession(session);
          setSessionReadError(null);
        },
        (caught) => {
          if (!active || httpStatus(caught) === 404) return;
          setSessionReadError(errorMessage(caught));
          timer = window.setTimeout(read, SESSION_MESSAGES_POLL_MS);
        },
      );
    };
    read();
    return () => {
      active = false;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [routeAgentId, routeSessionId, sessionListed]);
  const activeSession =
    listedSession ??
    (knownSession &&
    knownSession.id === routeSessionId &&
    knownSession.agentId === routeAgentId
      ? knownSession
      : null);
  // A helper's session shows the helper, not the companion.
  const sessionAgent =
    activeSession && agent && activeSession.agentId !== agent.id
      ? (agents.find((item) => item.id === activeSession.agentId) ?? agent)
      : agent;
  // The composer waits for the record: a send needs the session's kind.
  const sessionLoading = routeSessionId !== null && activeSession === null;
  const [messagesRefresh, setMessagesRefresh] = useState(0);
  const history = useSessionMessages(
    routeSessionId ? routeAgentId : null,
    routeSessionId,
    messagesRefresh,
    streamOpen ? SESSION_MESSAGES_LIVE_POLL_MS : SESSION_MESSAGES_POLL_MS,
  );
  const chatMessages = useMemo(
    () => history.messages.map(toChatMessage),
    [history.messages],
  );
  // One identity for the view's life, so a render of the page (a streamed
  // delta, a keystroke) keeps the message list's props.
  const loadOlderRef = useRef(history.loadOlder);
  loadOlderRef.current = history.loadOlder;
  const loadOlder = useCallback(() => void loadOlderRef.current(), []);

  const conversation = routeSessionId
    ? sessionConversation(routeSessionId)
    : HOME_CONVERSATION;
  const activeChatKey = agentId ? chatKey(agentId, conversation) : null;
  const [chats, setChats] = useState<Record<string, ChatState>>({});
  const chat = activeChatKey ? chatState(chats, activeChatKey) : EMPTY_CHAT;
  const { draft, failedDrafts, sending, error: workspaceError } = chat;
  const failedDraft = failedDrafts[0]?.text ?? null;
  const storedDraftsRef = useRef(new Map<string, string>());
  useEffect(() => {
    for (const [key, value] of Object.entries(chats)) {
      if (storedDraftsRef.current.get(key) === value.draft) continue;
      storedDraftsRef.current.set(key, value.draft);
      storeDraft(key, value.draft);
    }
  }, [chats]);
  /** A deleted session's chat state and saved draft go with it. */
  const forgetChat = (key: string) => {
    storedDraftsRef.current.delete(key);
    storeDraft(key, '');
    setChats((current) => {
      if (!(key in current)) return current;
      const rest = { ...current };
      delete rest[key];
      return rest;
    });
  };
  const updateChat = useCallback(
    (
      key: string,
      patch: Partial<ChatState> | ((value: ChatState) => Partial<ChatState>),
    ) => {
      setChats((current) => {
        const value = chatState(current, key);
        return {
          ...current,
          [key]: {
            ...value,
            ...(typeof patch === 'function' ? patch(value) : patch),
          },
        };
      });
    },
    [],
  );
  const setDraft = useCallback(
    (value: string | ((current: string) => string)) => {
      if (activeChatKey)
        updateChat(activeChatKey, (current) => ({
          draft: typeof value === 'function' ? value(current.draft) : value,
        }));
    },
    [activeChatKey, updateChat],
  );
  const setFailedDrafts = (
    value: (current: ChatState['failedDrafts']) => ChatState['failedDrafts'],
  ) => {
    if (activeChatKey)
      updateChat(activeChatKey, (current) => ({
        failedDrafts: value(current.failedDrafts),
      }));
  };
  const setWorkspaceError = (error: string | null) => {
    if (activeChatKey) updateChat(activeChatKey, { error });
  };
  /** Conversations whose new chat is still creating its session. */
  const pendingSendsRef = useRef(new Set<string>());
  // Messages go to the runs route (spec §4.2): the daemon queues them, so
  // the composer stays usable while the companion works.
  const sends = useSessionSends({
    // A send still unaccepted when the page last closed (S3b-A): offered
    // back in that chat's recovery panel, its key reused if it is resent.
    onRestore: (item) =>
      updateChat(item.conversation, (current) => ({
        failedDrafts: [
          ...current.failedDrafts,
          { requestId: item.key, text: item.text, idempotencyKey: item.key },
        ],
      })),
    onAccepted: (item, result) => {
      if (!availableAgentIdsRef.current.has(item.agentId)) return;
      if (item.telegram && !result.steer)
        updateChat(item.conversation, {
          delivery: { runId: result.run.id, queued: false },
        });
      // Its row shows at once, not only once the stream or ledger has it.
      if (!result.steer) live.seedRun(result.run);
      // A burst of sends settles into one read of each kind (S3b-F).
      live.refreshAllSoon();
    },
    onFailed: (item, caught) => {
      if (!availableAgentIdsRef.current.has(item.agentId)) return;
      const recover = () =>
        updateChat(item.conversation, (current) => ({
          failedDrafts: [
            ...current.failedDrafts,
            { requestId: item.key, text: item.text, idempotencyKey: item.key },
          ],
          error: item.telegram
            ? safeIntegrationError(caught)
            : errorMessage(caught),
        }));
      if (httpStatus(caught) !== 404) {
        recover();
        return;
      }
      // The runs route answers 404 for a session that is gone, and so does
      // a daemon without the route: the session itself tells them apart.
      daemon.getSession(item.agentId, item.sessionId).then(() => {
        if (!availableAgentIdsRef.current.has(item.agentId)) return;
        // It never reached a daemon that could take it: nothing goes to
        // recovery, and the text waits in the composer for the update.
        setRunsRouteMissing(true);
        updateChat(item.conversation, (current) => ({
          draft: current.draft.trim()
            ? `${current.draft}\n\n${item.text}`
            : item.text,
        }));
      }, recover);
    },
  });
  // The last Telegram reply's run, until it is known how it ended.
  const awaitedDelivery = chat.delivery?.queued
    ? null
    : (chat.delivery?.runId ?? null);
  const watchedRunIds = useMemo(
    () => (awaitedDelivery ? [awaitedDelivery] : []),
    [awaitedDelivery],
  );
  const openRoute =
    routeAgentId && routeSessionId
      ? { agentId: routeAgentId, sessionId: routeSessionId }
      : null;
  const refreshSessions = sessions.refresh;
  // The companion's one event stream (spec §6) and the open session's runs.
  const live = useLiveSession({
    agentId,
    session: openRoute,
    replyName: sessionAgent?.name ?? 'Your companion',
    listedActiveRuns: listedSession?.activeRuns ?? null,
    messages: history.messages,
    watchedRunIds,
    refreshSessions: () => void refreshSessions(),
    refreshMessages: () => setMessagesRefresh((value) => value + 1),
  });
  useEffect(() => {
    setStreamOpen(live.status === 'open');
  }, [live.status]);
  // A daemon that cannot take a send asks for an update (spec §13.4): one
  // without the sessions routes (pre-M2), or one with sessions but neither
  // the event stream nor the runs route (M2).
  const daemonTooOld =
    sessions.daemonTooOld || live.status === 'unsupported' || runsRouteMissing;
  const activeRun = live.activeRun;
  const openPending = useSessionPending({
    sends: sends.sends,
    settle: sends.settle,
    session: activeSession ? openRoute : null,
    messages: history.messages,
    runs: live.runs,
    ledger: live.ledger,
    appliedRead: history.appliedRead,
    readsStarted: history.readsStarted,
    refreshMessages: history.refresh,
    refreshRuns: live.refreshRuns,
    onRecover: (item) => {
      if (!availableAgentIdsRef.current.has(item.agentId)) return;
      // A new key when sent again: the steer's may still name its old run.
      updateChat(item.conversation, (current) => ({
        failedDrafts: [
          ...current.failedDrafts,
          { requestId: item.key, text: item.text },
        ],
      }));
    },
  });
  // A listing read before the stream's newest events does not keep
  // "is thinking" up: the open stream counts the active runs.
  const { activeRunCount } = live;
  const viewedSession = useMemo(
    () =>
      activeSession && activeRunCount !== null
        ? { ...activeSession, activeRuns: activeRunCount }
        : activeSession,
    [activeSession, activeRunCount],
  );
  const [settingsSaveError, setSettingsSaveError] = useState<string | null>(
    null,
  );
  const [resetError, setResetError] = useState<string | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  const [savingSettings, setSavingSettings] = useState(false);
  const [resetting, setResetting] = useState(false);
  const [legacyMigrationError, setLegacyMigrationError] = useState<
    string | null
  >(null);

  const savingSettingsRef = useRef(false);
  const agentOperationGenerationRef = useRef(0);
  const agentLifecycleGenerationRef = useRef(0);
  const settingsOperationGenerationRef = useRef<number | null>(null);
  const resetInFlightRef = useRef<AgentOperation | null>(null);
  const settingsTriggerRef = useRef<HTMLElement | null>(null);
  const currentAgentIdRef = useRef<string | null>(null);
  const previousSelectedMainIdRef = useRef<string | null>(null);

  const integrations = useAgentIntegrations(agentId);
  const telegramConnector = integrations.connectors[0] ?? null;
  const activeConnector =
    activeSession?.kind === 'telegram'
      ? (integrations.connectors.find(
          (item) => item.roomId === activeSession.roomId,
        ) ?? null)
      : null;
  // The connector queues a Telegram reply for delivery when its run
  // completes while it has an approved chat. How the run ended comes from
  // its lifecycle event or the ledger, which is read once its messages
  // arrive (useLiveSession's `watchedRunIds`).
  const deliveryApproved = activeConnector?.approvedChat != null;
  const deliveryRun = awaitedDelivery
    ? (live.runs.find((item) => item.run.id === awaitedDelivery)?.run ?? null)
    : null;
  useEffect(() => {
    if (
      !deliveryRun ||
      !activeChatKey ||
      !isTerminalRunStatus(deliveryRun.status)
    )
      return;
    const runId = deliveryRun.id;
    const queued = deliveryRun.status === 'completed' && deliveryApproved;
    updateChat(activeChatKey, (current) =>
      current.delivery?.runId === runId
        ? { delivery: queued ? { runId, queued } : null }
        : {},
    );
  }, [activeChatKey, deliveryRun, deliveryApproved, updateChat]);
  useLayoutEffect(() => {
    if (previousSelectedMainIdRef.current === agentId) return;

    const previousAgentId = previousSelectedMainIdRef.current;
    previousSelectedMainIdRef.current = agentId;
    agentLifecycleGenerationRef.current += 1;
    agentOperationGenerationRef.current += 1;
    currentAgentIdRef.current = agentId;
    settingsOperationGenerationRef.current = null;
    resetInFlightRef.current = null;
    settingsTriggerRef.current = null;
    savingSettingsRef.current = false;

    setLegacyMigrationError(null);
    setSessionActionError(null);
    setSettingsSaveError(null);
    setResetError(null);
    setShowSettings(false);
    setSavingSettings(false);
    setResetting(false);
    // Another companion has other sessions: start from a new chat.
    if (previousAgentId !== null) navigate({ kind: 'home' }, { replace: true });
  }, [agentId, navigate]);

  const beginAgentOperation = useCallback(
    (targetAgentId: string): AgentOperation => ({
      generation: ++agentOperationGenerationRef.current,
      lifecycleGeneration: agentLifecycleGenerationRef.current,
      targetAgentId,
    }),
    [],
  );

  const isCurrentAgentOperation = useCallback(
    (operation: AgentOperation) =>
      operation.generation === agentOperationGenerationRef.current &&
      operation.lifecycleGeneration === agentLifecycleGenerationRef.current &&
      operation.targetAgentId === currentAgentIdRef.current,
    [],
  );

  const isCurrentResetOperation = useCallback(
    (operation: AgentOperation) =>
      resetInFlightRef.current === operation &&
      operation.lifecycleGeneration === agentLifecycleGenerationRef.current &&
      operation.targetAgentId === currentAgentIdRef.current,
    [],
  );

  const adoptAgentSnapshot = useCallback(
    (operation: AgentOperation, snapshot: DaemonSnapshot) => {
      if (
        !isCurrentAgentOperation(operation) ||
        snapshot.state.id !== operation.targetAgentId
      ) {
        return false;
      }

      acceptAgentSnapshot(snapshot);
      return true;
    },
    [acceptAgentSnapshot, isCurrentAgentOperation],
  );

  const scrollerRef = useRef<HTMLDivElement>(null);
  const scrollDown = () => {
    requestAnimationFrame(() => {
      const element = scrollerRef.current;
      if (element) element.scrollTop = element.scrollHeight;
    });
  };

  useEffect(() => {
    if (!agentId) return;
    let current = true;
    void importLegacyCheckins(agentId)
      .then((result) => {
        if (!current) return;
        if (result.malformed > 0) {
          setLegacyMigrationError(
            `${result.malformed} legacy check-in record could not be imported and was kept in this browser.`,
          );
        } else if (result.imported > 0) {
          setLegacyMigrationError(null);
        }
      })
      .catch(() => {
        if (current)
          setLegacyMigrationError(
            'Legacy check-ins could not be imported. They remain in this browser for retry.',
          );
      });
    return () => {
      current = false;
    };
  }, [agentId]);

  // Opening an unread session marks it read up to its newest message, but
  // not while a page hides it.
  const markedReadRef = useRef(new Map<string, number>());
  const conversationHidden = availablePage(route) !== null;
  useEffect(() => {
    if (
      conversationHidden ||
      !activeSession?.unread ||
      history.messages.length === 0
    )
      return;
    const newest = history.messages[history.messages.length - 1].createdAtMs;
    const key = sessionKey(activeSession);
    if ((markedReadRef.current.get(key) ?? 0) >= newest) return;
    markedReadRef.current.set(key, newest);
    void daemon
      .updateSession(activeSession.agentId, activeSession.id, {
        lastReadAtMs: newest,
      })
      .then(
        () => refreshSessions(),
        () => markedReadRef.current.delete(key),
      );
  }, [activeSession, conversationHidden, history.messages, refreshSessions]);

  const changeWorkspaceAvatar = useCallback(
    async (file: File) => {
      await daemon.uploadWorkspaceAvatar(file);
      await refreshWorkspace();
    },
    [refreshWorkspace],
  );

  const openSettings = () => {
    settingsTriggerRef.current =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    // Summaries carry no transcripts: Settings reads the full records.
    if (streamOpen) void refreshAgents();
    setShowSettings(true);
  };
  const closeSettings = () => {
    if (savingSettingsRef.current || resetInFlightRef.current !== null) return;
    setShowSettings(false);
  };

  useEffect(() => {
    if (showSettings || !settingsTriggerRef.current) return;
    const trigger = settingsTriggerRef.current;
    settingsTriggerRef.current = null;
    trigger.focus();
  }, [showSettings]);

  const saveSettings = async (patch: AgentUpdateInput): Promise<boolean> => {
    if (
      !agent ||
      savingSettingsRef.current ||
      resetInFlightRef.current !== null
    ) {
      return false;
    }
    const operation = beginAgentOperation(agent.id);
    settingsOperationGenerationRef.current = operation.generation;
    savingSettingsRef.current = true;
    setSavingSettings(true);
    setSettingsSaveError(null);
    setResetError(null);
    try {
      const { agent: updatedAgent } = await daemon.updateAgent(agent.id, patch);
      const adopted = adoptAgentSnapshot(operation, updatedAgent);
      if (adopted) {
        setSettingsSaveError(null);
      }
      return adopted;
    } catch (caught) {
      if (isCurrentAgentOperation(operation)) {
        setSettingsSaveError(errorMessage(caught));
      }
      return false;
    } finally {
      if (settingsOperationGenerationRef.current === operation.generation) {
        settingsOperationGenerationRef.current = null;
        savingSettingsRef.current = false;
        setSavingSettings(false);
      }
    }
  };

  const resetAgent = async () => {
    if (
      !agent ||
      savingSettingsRef.current ||
      resetInFlightRef.current !== null
    ) {
      return;
    }
    const targetAgentId = agent.id;
    const operation = beginAgentOperation(targetAgentId);
    resetInFlightRef.current = operation;
    setResetting(true);
    setResetError(null);
    setSettingsSaveError(null);
    try {
      try {
        await daemon.deleteAgent(targetAgentId);
      } catch (caught) {
        if (isCurrentResetOperation(operation)) {
          setResetError(errorMessage(caught));
        }
        return;
      }

      const ownsSelectedMain = isCurrentResetOperation(operation);
      if (ownsSelectedMain) {
        agentLifecycleGenerationRef.current += 1;
        agentOperationGenerationRef.current += 1;
        currentAgentIdRef.current = null;
      }
      availableAgentIdsRef.current.delete(targetAgentId);
      removeAgentSnapshot(targetAgentId);
      sends.forgetAgent(targetAgentId);
      try {
        clearCheckins(targetAgentId);
      } catch {
        // The daemon deletion is authoritative; local cleanup is best-effort.
      }
    } finally {
      if (resetInFlightRef.current === operation) {
        resetInFlightRef.current = null;
        setResetting(false);
      }
    }
  };

  /** Hands a message to the send queue (spec §4.2): retried with its key,
   *  in order with the session's other messages. */
  const queueSend = (
    target: Pick<Session, 'agentId' | 'id' | 'kind'>,
    conversation: string,
    text: string,
    idempotencyKey: string,
    mode: RunMode = 'queue',
  ) => {
    if (
      !availableAgentIdsRef.current.has(target.agentId) ||
      resetInFlightRef.current !== null
    )
      return;
    sends.send({
      key: idempotencyKey,
      agentId: target.agentId,
      sessionId: target.id,
      conversation,
      text,
      mode,
      telegram: target.kind === 'telegram',
    });
  };

  /** A new chat becomes a session with its first message (spec §3.3). */
  const startChat = async (targetId: string, text: string) => {
    const homeKey = chatKey(targetId, HOME_CONVERSATION);
    if (pendingSendsRef.current.has(homeKey)) return;
    pendingSendsRef.current.add(homeKey);
    updateChat(homeKey, { sending: true, error: null, draft: '' });
    let session: Session;
    try {
      session = await daemon.createSession(targetId);
    } catch (caught) {
      pendingSendsRef.current.delete(homeKey);
      updateChat(homeKey, (current) => ({
        sending: false,
        failedDrafts: [
          ...current.failedDrafts,
          { requestId: crypto.randomUUID(), text },
        ],
        error: errorMessage(caught),
      }));
      return;
    }
    pendingSendsRef.current.delete(homeKey);
    if (currentAgentIdRef.current !== targetId) {
      updateChat(homeKey, { sending: false });
      return;
    }
    const target = chatKey(targetId, sessionConversation(session.id));
    // Text typed while the chat was created moves with it.
    setChats((current) => {
      const home = chatState(current, homeKey);
      return {
        ...current,
        [homeKey]: { ...home, draft: '', sending: false },
        [target]: { ...chatState(current, target), draft: home.draft },
      };
    });
    sessions.upsert(session);
    const created: HashRoute = { kind: 'session', sessionId: session.id };
    // Follow the new chat only while it is still the conversation on screen
    // or behind a page: a session opened meanwhile keeps the owner, a page
    // that hides the chat stays open with the session behind it, and a page
    // that shows the chat moves to the session so a reload finds it.
    if (lastConversationRef.current.kind === 'home') {
      if (availablePage(routeRef.current) !== null)
        lastConversationRef.current = created;
      else navigate(created, { replace: true });
    }
    queueSend(session, target, text, crypto.randomUUID());
  };

  const newChat = () => navigate({ kind: 'home' });
  const renameSession = async (session: Session, title: string) => {
    try {
      await daemon.updateSession(session.agentId, session.id, { title });
      setSessionActionError(null);
      await sessions.refresh();
      return true;
    } catch (caught) {
      setSessionActionError(errorMessage(caught));
      return false;
    }
  };
  const archiveSession = async (session: Session, archived: boolean) => {
    try {
      await daemon.updateSession(session.agentId, session.id, { archived });
      setSessionActionError(null);
      await sessions.refresh();
      return true;
    } catch (caught) {
      setSessionActionError(errorMessage(caught));
      return false;
    }
  };
  const exportSession = async (session: Session) => {
    try {
      saveTextFile(
        exportFileName(session.title),
        await daemon.exportSession(session.agentId, session.id),
      );
      setSessionActionError(null);
    } catch (caught) {
      setSessionActionError(errorMessage(caught));
    }
  };
  const deleteSession = async (session: Session) => {
    try {
      await daemon.deleteSession(session.agentId, session.id);
      setSessionActionError(null);
      sessions.remove(session);
      if (agentId)
        forgetChat(chatKey(agentId, sessionConversation(session.id)));
      // Judge by the route now: a page or another session may have opened meanwhile.
      const open = lastConversationRef.current;
      if (open.kind === 'session' && open.sessionId === session.id) {
        if (routeRef.current.kind === 'page')
          lastConversationRef.current = { kind: 'home' };
        else navigate({ kind: 'home' }, { replace: true });
      }
      return true;
    } catch (caught) {
      setSessionActionError(errorMessage(caught));
      return false;
    }
  };

  const commands = useSessionCommands({
    companionId: agentId,
    canSend: () =>
      agent !== null &&
      connection === 'online' &&
      resetInFlightRef.current === null &&
      !daemonTooOld,
    routeSessionId,
    session: activeSession,
    chatKey: activeChatKey,
    draft,
    resend: chat.resend,
    telegramReady: activeConnector !== null,
    activeRun,
    updateChat,
    startChat: (text) => {
      if (agent) void startChat(agent.id, text);
    },
    queueSend,
    refreshRuns: live.refreshRuns,
    setError: setWorkspaceError,
    navigate,
    listedSessions: sessions.sessions,
    upsertSession: sessions.upsert,
    setKnownSession,
    newChat,
    showCommands: () => setDraft('/'),
    search: setSessionQuery,
    chooseModel: openSettings,
    rename: renameSession,
    archive: archiveSession,
    exportSession,
  });
  const { stopRun } = commands;
  const openSession = (session: Session) =>
    commands.openTarget({ agentId: session.agentId, sessionId: session.id });
  const transcriptActions = useTranscriptActions({
    session: activeSession,
    // A Telegram session's messages need its connector.
    resendable: activeSession?.kind !== 'telegram' || activeConnector !== null,
    liveRuns: live.state.runs,
    sessions: sessions.sessions,
    stopRun: (run) => void stopRun(run),
    sendAgain: commands.sendAgain,
    compact: (session) => void commands.compactSession(session),
    compacting: commands.compacting,
    openSession: commands.openTarget,
  });

  if (connection === 'unknown' || (connection === 'online' && !loaded)) {
    return <ConnectingState />;
  }

  if (!agent && connection === 'offline') {
    return <OfflineRetry retry={refreshAgents} />;
  }

  const onboardingLifecycleGeneration = agentLifecycleGenerationRef.current;
  if (!agent) {
    return (
      <CompanionSetup
        providers={providers}
        providersError={providersError}
        retryProviders={retryProviders}
        onCreated={(snapshot) => {
          if (
            currentAgentIdRef.current !== null ||
            agentLifecycleGenerationRef.current !==
              onboardingLifecycleGeneration
          ) {
            return;
          }
          agentOperationGenerationRef.current += 1;
          agentLifecycleGenerationRef.current += 1;
          acceptAgentSnapshot(snapshot);
          void refreshWorkspace();
          scrollDown();
        }}
      />
    );
  }

  const settingsPanel = showSettings ? (
    <SettingsPanel
      refreshProviders={retryProviders}
      agent={agent}
      providers={providers}
      workspace={workspace}
      saving={savingSettings}
      resetting={resetting}
      saveError={settingsSaveError}
      resetError={resetError}
      saveSettings={saveSettings}
      resetAgent={resetAgent}
      close={closeSettings}
    />
  ) : null;

  const sidebar = (
    <SessionSidebar
      sessions={sessions.sessions}
      activeKey={activeSession ? sessionKey(activeSession) : null}
      query={sessionQuery}
      onQueryChange={setSessionQuery}
      showArchived={showArchived}
      onShowArchivedChange={setShowArchived}
      error={sessionActionError ?? (daemonTooOld ? null : sessions.error)}
      hasMore={sessions.hasMore}
      loadingMore={sessions.loadingMore}
      onLoadMore={() => void sessions.loadMore()}
      onOpen={openSession}
      onRename={renameSession}
      onArchive={archiveSession}
      onExport={exportSession}
      onDelete={deleteSession}
    />
  );

  // A helper session's user turns came from the agent that delegated or
  // sent them (M2 final review).
  const delegatedBy =
    activeSession?.kind === 'helper'
      ? (agents.find((item) => item.id === activeSession.parentAgentId)?.name ??
        agent.name)
      : null;

  const sessionView = (
    <SessionView
      agent={sessionAgent ?? agent}
      session={viewedSession}
      messages={routeSessionId ? chatMessages : []}
      pending={openPending}
      runs={live.runs}
      actions={transcriptActions}
      delegatedBy={delegatedBy}
      announcement={live.announcement}
      hasOlder={history.hasOlder}
      loadingOlder={history.loadingOlder}
      onLoadOlder={loadOlder}
      missing={routeSessionId !== null && history.missing && !daemonTooOld}
      telegramAvailable={activeConnector !== null}
      scrollerRef={scrollerRef}
      onSuggestion={setDraft}
      composer={{
        draft,
        setDraft,
        sending,
        // Usable while the companion works: messages queue (spec §15.3).
        disabled: resetting || sessionLoading || daemonTooOld,
        offline: connection === 'offline',
        onSend: commands.send,
        error: workspaceError,
        onDismissError: () => setWorkspaceError(null),
        commands: SLASH_COMMANDS,
        runActive: activeRun !== null,
        onStop:
          activeRun && activeSession?.capabilities.stop
            ? () => void stopRun(activeRun)
            : undefined,
        onSteer:
          activeRun && activeSession?.capabilities.steer
            ? commands.steer
            : undefined,
        recovery:
          failedDraft && !sending
            ? {
                count: failedDrafts.length,
                text: failedDraft,
                restore: () => {
                  if (!activeChatKey) return;
                  updateChat(activeChatKey, (current) => {
                    const [first, ...rest] = current.failedDrafts;
                    if (!first) return {};
                    return {
                      draft: current.draft.trim()
                        ? `${current.draft}\n\n${first.text}`
                        : first.text,
                      failedDrafts: rest,
                      resend: first.idempotencyKey
                        ? {
                            text: first.text,
                            idempotencyKey: first.idempotencyKey,
                          }
                        : current.resend,
                    };
                  });
                },
                dismiss: () => setFailedDrafts((current) => current.slice(1)),
              }
            : undefined,
      }}
      onNewChat={newChat}
      onOpenWork={() => navigate({ kind: 'page', page: 'work' })}
      onRename={(title) =>
        activeSession
          ? renameSession(activeSession, title)
          : Promise.resolve(false)
      }
      onToggleArchived={() => {
        if (activeSession)
          void archiveSession(activeSession, !activeSession.archived);
      }}
      onExport={() => {
        if (activeSession) void exportSession(activeSession);
      }}
      notice={
        <>
          {legacyMigrationError ? (
            <p role="status" className="px-4 pt-3 text-xs text-ink-3">
              {legacyMigrationError}
            </p>
          ) : null}
          {daemonTooOld ? (
            <div
              role="alert"
              className="mx-4 mt-3 rounded-xl border border-danger/25 bg-danger/[0.08] px-3.5 py-2.5 text-xs leading-relaxed"
            >
              <p className="font-semibold text-danger">Update the daemon</p>
              <p className="text-ink-2">
                {sessions.daemonTooOld
                  ? 'This console keeps chats as sessions'
                  : 'This console sends messages as live runs'}
                , which this anima-daemon does not support yet. Update and
                restart the daemon, then reload this page.
              </p>
            </div>
          ) : null}
          {/* A dropped stream says so until its next snapshot (spec §16);
           *  the region stays, so a retry that fails again reads nothing. */}
          <div role="status">
            {live.status === 'reconnecting' ? (
              <p className="px-4 pt-3 text-center font-mono text-[10px] text-ink-3">
                Reconnecting…
              </p>
            ) : null}
          </div>
          {chat.delivery?.queued ? (
            <p
              role="status"
              className="px-4 pt-3 text-center font-mono text-[10px] text-mint"
            >
              Queued for Telegram delivery
            </p>
          ) : null}
          {sessionActionError ? (
            <p role="alert" className="px-4 pt-3 text-xs text-danger">
              {sessionActionError}
            </p>
          ) : null}
          {sessionLoading && sessionReadError ? (
            <p role="alert" className="px-4 pt-3 text-xs text-danger">
              Session details could not be loaded: {sessionReadError}. Retrying…
            </p>
          ) : null}
          {routeSessionId && history.error ? (
            <p role="alert" className="px-4 pt-3 text-xs text-danger">
              Messages could not be loaded: {history.error}
            </p>
          ) : null}
        </>
      }
    />
  );

  return (
    <>
      <div
        data-testid="workspace-background"
        className="contents"
        aria-hidden={showSettings || undefined}
        inert={showSettings || undefined}
      >
        <WorkspaceShell
          mainAgent={mainAgent ?? agent}
          agents={agents}
          connection={connection}
          route={route}
          navigate={navigate}
          onNewChat={newChat}
          onOpenSettings={openSettings}
          onChangeWorkspaceAvatar={changeWorkspaceAvatar}
          onPickPrompt={(prompt) =>
            setDraft((current) =>
              current.trim() ? `${current}\n\n${prompt}` : prompt,
            )
          }
          connectors={
            <ConnectorsView
              agentId={agent.id}
              telegram={
                <TelegramSettings
                  connector={telegramConnector}
                  busy={integrations.connectorBusy}
                  error={integrations.connectorError}
                  connect={integrations.connectTelegram}
                  replace={integrations.replaceTelegram}
                  approve={integrations.approvePairing}
                  restart={integrations.restartTelegram}
                  disconnect={integrations.disconnectTelegram}
                  refresh={integrations.refresh}
                />
              }
            />
          }
          workspaceState={workspace}
          sidebar={sidebar}
          conversation={sessionView}
          conversationRoute={conversationRoute}
        />
      </div>
      {settingsPanel}
    </>
  );
}
