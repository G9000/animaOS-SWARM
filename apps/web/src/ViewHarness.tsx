import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import type { Session, SessionMessage } from '@animaOS-SWARM/sdk';

import { AlertIcon } from './components/icons';
import { CompanionSetup } from './components/onboarding/CompanionSetup';
import { SettingsPanel } from './components/SettingsPanel';
import { ConnectorsView } from './components/ConnectorsView';
import { SessionSidebar } from './components/sessions/SessionSidebar';
import { SessionView } from './components/sessions/SessionView';
import { TelegramSettings } from './components/TelegramSettings';
import { WorkspaceShell } from './components/WorkspaceShell';
import { useAgentIntegrations } from './hooks/useAgentIntegrations';
import { useCompanionSessions } from './hooks/useCompanionSessions';
import { useDaemonBootstrap } from './hooks/useDaemonBootstrap';
import {
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
import {
  createTelegramIdempotencyKey,
  safeIntegrationError,
} from './lib/telegram';

interface AgentOperation {
  generation: number;
  lifecycleGeneration: number;
  targetAgentId: string;
}

type ChatState = {
  draft: string;
  failedDrafts: { requestId: string; text: string }[];
  sending: boolean;
  error: string | null;
};

const EMPTY_CHAT: ChatState = {
  draft: '',
  failedDrafts: [],
  sending: false,
  error: null,
};
const HOME_CONVERSATION = 'home';
/** The daemon's largest message page, so a busy session still shows the request. */
const REQUEST_CHECK_PAGE = 200;

/** A send whose outcome is unknown: its request failed in transit or timed out. */
interface UncertainSend {
  agentId: string;
  sessionId: string;
  key: string;
  text: string;
  /** Timed out: the run may still be queued or running, so the chat stays locked. */
  waiting: boolean;
}

/** What the send's session said when it was last read again. */
interface SendCheck {
  activeRuns: number;
  delivered: boolean;
}

/** A committed user message with this request ID means its blocking run
 *  finished (M2 runs commit their messages together). */
function carriesRequest(message: SessionMessage, requestId: string): boolean {
  return (
    message.role === 'user' && message.metadata.clientRequestId === requestId
  );
}

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

// Drafts are saved per agent and conversation in session storage (spec
// §15.5), so a reload keeps them. Without storage they live in memory only.
function draftStorageKey(key: string): string {
  return `animaos.draft.${key.replace('\u0000', '/')}`;
}

function loadDraft(key: string): string {
  try {
    return window.sessionStorage.getItem(draftStorageKey(key)) ?? '';
  } catch {
    return '';
  }
}

function storeDraft(key: string, draft: string) {
  try {
    if (draft) window.sessionStorage.setItem(draftStorageKey(key), draft);
    else window.sessionStorage.removeItem(draftStorageKey(key));
  } catch {
    // Storage is full or blocked: the draft stays in memory for this page.
  }
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
  } = useDaemonBootstrap();
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
  const sessions = useCompanionSessions(agentId, {
    archived: showArchived,
    query: sessionQuery,
  });
  // A daemon without the sessions routes cannot take a send (spec §13.4).
  const daemonTooOld = sessions.daemonTooOld;
  const routeSessionId =
    conversationRoute.kind === 'session' ? conversationRoute.sessionId : null;
  const listedSession = routeSessionId
    ? (sessions.sessions.find((item) => item.id === routeSessionId) ?? null)
    : null;
  const sessionListed = listedSession !== null;
  // A session outside the loaded list (archived, older, or filtered out by a
  // search) keeps its last known record and is read again on its own.
  const [knownSession, setKnownSession] = useState<Session | null>(null);
  const [sessionReadError, setSessionReadError] = useState<string | null>(
    null,
  );
  useEffect(() => {
    if (listedSession) setKnownSession(listedSession);
  }, [listedSession]);
  useEffect(() => {
    setSessionReadError(null);
    if (!routeSessionId || sessionListed || !agentId) return;
    let active = true;
    let timer: number | undefined;
    // A failed read is retried on the messages' cadence while the route
    // points here; a missing session shows as deleted through its messages.
    const read = () => {
      daemon.getSession(agentId, routeSessionId).then(
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
  }, [agentId, routeSessionId, sessionListed]);
  const activeSession =
    listedSession ??
    (knownSession && knownSession.id === routeSessionId ? knownSession : null);
  // The composer waits for the record: a send needs the session's room.
  const sessionLoading = routeSessionId !== null && activeSession === null;
  const [messagesRefresh, setMessagesRefresh] = useState(0);
  const history = useSessionMessages(
    routeSessionId ? (activeSession?.agentId ?? agentId) : null,
    routeSessionId,
    messagesRefresh,
  );
  const chatMessages = useMemo(
    () => history.messages.map(toChatMessage),
    [history.messages],
  );

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
  const pendingSendsRef = useRef(new Set<string>());
  const uncertainSendsRef = useRef(new Map<string, UncertainSend>());
  // A timed-out send is settled from its own session, never from the agent's
  // status: that also reads "running" for a check-in or Telegram turn in
  // another session, and it can read idle while this request is still
  // queued behind another run in its room (not yet in the run ledger).
  const sendChecksRef = useRef(new Map<string, SendCheck>());
  const [sendCheckRevision, setSendCheckRevision] = useState(0);
  const mountedRef = useRef(true);
  const checkTimersRef = useRef(new Set<number>());
  useEffect(() => {
    mountedRef.current = true;
    const timers = checkTimersRef.current;
    return () => {
      mountedRef.current = false;
      for (const timer of timers) window.clearTimeout(timer);
      timers.clear();
    };
  }, []);
  /** Reads a timed-out send's session and then its messages, again while the
   *  session has active runs. Reading the session first means a page read
   *  after it reports none already holds what those runs committed. */
  const checkUncertainSend = async (requestId: string) => {
    const pending = uncertainSendsRef.current.get(requestId);
    if (!pending?.waiting || !mountedRef.current) return;
    let check: SendCheck | null = null;
    try {
      const session = await daemon.getSession(
        pending.agentId,
        pending.sessionId,
      );
      const page = await daemon.sessionMessages(
        pending.agentId,
        pending.sessionId,
        { limit: REQUEST_CHECK_PAGE },
      );
      check = {
        activeRuns: session.activeRuns,
        delivered: page.messages.some((message) =>
          carriesRequest(message, requestId),
        ),
      };
    } catch (caught) {
      // A deleted session has nothing left to wait for; other failures retry.
      if (httpStatus(caught) === 404) check = { activeRuns: 0, delivered: false };
    }
    if (
      !mountedRef.current ||
      uncertainSendsRef.current.get(requestId) !== pending
    )
      return;
    if (check) {
      sendChecksRef.current.set(requestId, check);
      setSendCheckRevision((value) => value + 1);
      if (check.delivered || check.activeRuns === 0) return;
    }
    const timer = window.setTimeout(() => {
      checkTimersRef.current.delete(timer);
      void checkUncertainSend(requestId);
    }, SESSION_MESSAGES_POLL_MS);
    checkTimersRef.current.add(timer);
  };
  useEffect(() => {
    for (const [requestId, pending] of uncertainSendsRef.current) {
      const snapshot = agentSnapshots.find(
        (item) => item.state.id === pending.agentId,
      );
      if (!snapshot) continue;
      const check = sendChecksRef.current.get(requestId);
      const delivered =
        check?.delivered === true ||
        snapshot.messages.some(
          (message) =>
            message.role === 'user' &&
            message.content.metadata?.clientRequestId === requestId,
        ) ||
        history.messages.some((message) => carriesRequest(message, requestId));
      // A timed-out send stays locked until its session has no active runs.
      if (!delivered && (!pending.waiting || !check || check.activeRuns > 0))
        continue;
      sendChecksRef.current.delete(requestId);
      if (delivered) uncertainSendsRef.current.delete(requestId);
      else
        uncertainSendsRef.current.set(requestId, {
          ...pending,
          waiting: false,
        });
      if (pending.waiting) pendingSendsRef.current.delete(pending.key);
      updateChat(pending.key, (current) => {
        const index = delivered
          ? current.failedDrafts.findIndex(
              (item) => item.requestId === requestId,
            )
          : -1;
        const remaining = current.failedDrafts.filter(
          (_, position) => position !== index,
        );
        return {
          failedDrafts: remaining,
          sending: pending.waiting ? false : current.sending,
          error:
            current.sending && !pending.waiting
              ? current.error
              : delivered
                ? remaining.length
                  ? current.error
                  : null
                : 'The daemon has not confirmed this message. Check the conversation before restoring it.',
        };
      });
      if (delivered) setMessagesRefresh((value) => value + 1);
    }
  }, [agentSnapshots, history.messages, sendCheckRevision, updateChat]);
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
          void integrations.refresh();
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

  // Opening an unread session marks it read up to its newest message.
  const markedReadRef = useRef(new Map<string, number>());
  const refreshSessions = sessions.refresh;
  useEffect(() => {
    if (!activeSession?.unread || history.messages.length === 0) return;
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
  }, [activeSession, history.messages, refreshSessions]);

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

  const refreshConversation = () => {
    setMessagesRefresh((value) => value + 1);
    void sessions.refresh();
  };

  /** One blocking run in a session's room (spec §4.9). */
  const runInSession = async (
    targetId: string,
    session: Pick<Session, 'id' | 'roomId'>,
    text: string,
    key: string,
    preserveDraft = false,
  ) => {
    if (
      !availableAgentIdsRef.current.has(targetId) ||
      connection !== 'online' ||
      pendingSendsRef.current.has(key) ||
      resetInFlightRef.current !== null
    )
      return;
    const clientRequestId = crypto.randomUUID();
    pendingSendsRef.current.add(key);
    updateChat(key, {
      sending: true,
      error: null,
      ...(preserveDraft ? {} : { draft: '' }),
    });
    try {
      const { agent: updatedAgent, result } = await daemon.runAgent(
        targetId,
        text,
        { clientRequestId },
        session.roomId,
      );
      if (
        availableAgentIdsRef.current.has(targetId) &&
        updatedAgent.state.id === targetId
      ) {
        acceptAgentSnapshot(updatedAgent);
        if (result.status === 'error')
          updateChat(key, { error: result.error ?? 'run failed' });
      }
    } catch (caught) {
      if (availableAgentIdsRef.current.has(targetId)) {
        const timedOut =
          caught instanceof Error && 'status' in caught && caught.status === 408;
        uncertainSendsRef.current.set(clientRequestId, {
          agentId: targetId,
          sessionId: session.id,
          key,
          text,
          waiting: timedOut,
        });
        updateChat(key, (current) => ({
          failedDrafts: [
            ...current.failedDrafts,
            { requestId: clientRequestId, text },
          ],
          error: timedOut
            ? 'The response timed out. Checking the daemon for completion—do not resend yet.'
            : errorMessage(caught),
        }));
        if (timedOut) {
          void refreshAgents();
          void checkUncertainSend(clientRequestId);
        }
      }
    } finally {
      if (!uncertainSendsRef.current.get(clientRequestId)?.waiting) {
        pendingSendsRef.current.delete(key);
        updateChat(key, { sending: false });
      }
      refreshConversation();
    }
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
    // A page opened meanwhile stays open; the new session waits behind it.
    if (routeRef.current.kind === 'page') lastConversationRef.current = created;
    else navigate(created, { replace: true });
    await runInSession(targetId, session, text, target, true);
  };

  /** An owner turn in a Telegram session goes out through its connector. */
  const replyOnTelegram = async (
    targetId: string,
    connectorId: string,
    text: string,
    key: string,
  ) => {
    if (pendingSendsRef.current.has(key)) return;
    pendingSendsRef.current.add(key);
    updateChat(key, { sending: true, error: null, draft: '' });
    try {
      const response = await daemon.sendConnectorMessage(
        targetId,
        connectorId,
        text,
        createTelegramIdempotencyKey(),
      );
      if (response.result.status === 'error')
        updateChat(key, { error: response.result.error ?? 'run failed' });
    } catch (caught) {
      updateChat(key, (current) => ({
        failedDrafts: [
          ...current.failedDrafts,
          { requestId: crypto.randomUUID(), text },
        ],
        error: safeIntegrationError(caught),
      }));
    } finally {
      pendingSendsRef.current.delete(key);
      updateChat(key, { sending: false });
      refreshConversation();
    }
  };

  const send = () => {
    if (
      !agent ||
      connection !== 'online' ||
      resetInFlightRef.current !== null ||
      daemonTooOld
    )
      return;
    const text = draft.trim();
    if (!text) return;
    if (!routeSessionId) {
      void startChat(agent.id, text);
      return;
    }
    // Until its record loads, the session's room and kind are unknown.
    if (!activeSession) return;
    const key = chatKey(agent.id, sessionConversation(routeSessionId));
    if (activeSession.kind === 'telegram') {
      if (activeConnector)
        void replyOnTelegram(agent.id, activeConnector.id, text, key);
      return;
    }
    void runInSession(activeSession.agentId, activeSession, text, key);
  };

  const newChat = () => navigate({ kind: 'home' });
  const openSession = (session: Session) =>
    navigate({ kind: 'session', sessionId: session.id });
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
    } catch (caught) {
      setSessionActionError(errorMessage(caught));
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
      if (agentId) forgetChat(chatKey(agentId, sessionConversation(session.id)));
      // Judge by the route now: a page or another session may have opened meanwhile.
      const open = lastConversationRef.current;
      if (open.kind === 'session' && open.sessionId === session.id) {
        if (routeRef.current.kind === 'page')
          lastConversationRef.current = { kind: 'home' };
        else navigate({ kind: 'home' }, { replace: true });
      }
    } catch (caught) {
      setSessionActionError(errorMessage(caught));
    }
  };

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
      onOpen={openSession}
      onRename={renameSession}
      onArchive={archiveSession}
      onExport={exportSession}
      onDelete={deleteSession}
    />
  );

  const sessionView = (
    <SessionView
      agent={agent}
      session={activeSession}
      messages={routeSessionId ? chatMessages : []}
      hasOlder={history.hasOlder}
      loadingOlder={history.loadingOlder}
      onLoadOlder={() => void history.loadOlder()}
      missing={routeSessionId !== null && history.missing && !daemonTooOld}
      telegramAvailable={activeConnector !== null}
      scrollerRef={scrollerRef}
      onSuggestion={setDraft}
      composer={{
        draft,
        setDraft,
        sending,
        disabled:
          resetting ||
          sessionLoading ||
          daemonTooOld ||
          (activeSession?.activeRuns ?? 0) > 0,
        offline: connection === 'offline',
        onSend: send,
        error: workspaceError,
        onDismissError: () => setWorkspaceError(null),
        recovery:
          failedDraft && !sending
            ? {
                count: failedDrafts.length,
                text: failedDraft,
                restore: () => {
                  setDraft((current) =>
                    current.trim()
                      ? `${current}\n\n${failedDraft}`
                      : failedDraft,
                  );
                  setFailedDrafts((current) => current.slice(1));
                },
                dismiss: () => setFailedDrafts((current) => current.slice(1)),
              }
            : undefined,
      }}
      onNewChat={newChat}
      onOpenWork={() => navigate({ kind: 'page', page: 'work' })}
      onRename={(title) =>
        activeSession ? renameSession(activeSession, title) : Promise.resolve(false)
      }
      onToggleArchived={() => {
        if (activeSession) void archiveSession(activeSession, !activeSession.archived);
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
                This console keeps chats as sessions, which this anima-daemon
                does not support yet. Update and restart the daemon, then
                reload this page.
              </p>
            </div>
          ) : null}
          {sessionLoading && sessionReadError ? (
            <p role="alert" className="px-4 pt-3 text-xs text-danger">
              Session details could not be loaded: {sessionReadError}.
              Retrying…
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
