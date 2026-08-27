import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from 'react';

import { ActivityView } from './components/ActivityView';
import { Composer, MessageList } from './components/ChatScreen';
import { AlertIcon } from './components/icons';
import { OnboardingFlow } from './components/onboarding/OnboardingFlow';
import { SettingsPanel } from './components/SettingsPanel';
import { WorkspaceShell } from './components/WorkspaceShell';
import { useDaemonBootstrap } from './hooks/useDaemonBootstrap';
import {
  CHECKIN_SENTINEL,
  clearCheckins,
  isDue,
  loadCheckins,
  newCheckin,
  saveCheckins,
  wrapPrompt,
  type Checkin,
} from './lib/checkins';
import { daemon, toAgentDetail, type DaemonSnapshot } from './lib/daemon-api';
import { selectMainAgent } from './lib/agent-access';

interface AgentOperation {
  generation: number;
  lifecycleGeneration: number;
  targetAgentId: string;
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
              className="typing-dot h-2 w-2 rounded-full bg-sky-400"
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
        className="glass-strong w-full max-w-lg rounded-3xl p-7 text-center"
      >
        <div className="mx-auto flex h-12 w-12 items-center justify-center rounded-full bg-red-400/10 text-red-300">
          <AlertIcon size={20} />
        </div>
        <h1 className="mt-4 font-display text-2xl font-semibold tracking-tight text-ink">
          Offline
        </h1>
        <p className="mx-auto mt-2 max-w-sm text-sm leading-relaxed text-ink-2">
          The workspace cannot reach anima-daemon yet. Start the Rust host, then
          retry this connection.
        </p>
        <code className="mt-4 inline-block rounded-xl border border-line bg-black/20 px-3 py-2 font-mono text-xs text-sky-300">
          bun dev --host rust
        </code>
        <div className="mt-5">
          <button
            type="button"
            autoFocus
            onClick={() => void retry()}
            className="rounded-xl bg-sky-500 px-4 py-2 text-sm font-semibold text-white transition hover:bg-sky-400"
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
    refreshAgents,
    retryProviders,
    acceptAgentSnapshot,
    removeAgentSnapshot,
  } = useDaemonBootstrap();
  const agents = useMemo(
    () => agentSnapshots.map((snapshot) => toAgentDetail(snapshot)),
    [agentSnapshots],
  );
  const agent = selectMainAgent(agents);

  const [draft, setDraft] = useState('');
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  const [savingSettings, setSavingSettings] = useState(false);
  const [resetting, setResetting] = useState(false);
  const [checkins, setCheckins] = useState<Checkin[]>([]);
  const [ciPrompt, setCiPrompt] = useState('');
  const [ciIntervalMin, setCiIntervalMin] = useState(30);

  const sendingRef = useRef(false);
  const savingSettingsRef = useRef(false);
  const agentOperationGenerationRef = useRef(0);
  const agentLifecycleGenerationRef = useRef(0);
  const sendingOperationGenerationRef = useRef<number | null>(null);
  const settingsOperationGenerationRef = useRef<number | null>(null);
  const resetInFlightRef = useRef<AgentOperation | null>(null);
  const currentAgentIdRef = useRef<string | null>(null);
  const previousSelectedMainIdRef = useRef<string | null>(null);
  const checkinsRef = useRef<Checkin[]>([]);
  const checkinRunningRef = useRef(false);
  const checkinRunGenerationRef = useRef(0);
  sendingRef.current = sending;
  checkinsRef.current = checkins;

  const agentId = agent?.id ?? null;
  useLayoutEffect(() => {
    if (previousSelectedMainIdRef.current === agentId) return;

    previousSelectedMainIdRef.current = agentId;
    agentLifecycleGenerationRef.current += 1;
    agentOperationGenerationRef.current += 1;
    currentAgentIdRef.current = agentId;
    sendingOperationGenerationRef.current = null;
    settingsOperationGenerationRef.current = null;
    resetInFlightRef.current = null;
    checkinRunGenerationRef.current += 1;
    checkinRunningRef.current = false;
    sendingRef.current = false;
    savingSettingsRef.current = false;

    setDraft('');
    setCiPrompt('');
    setCiIntervalMin(30);
    setError(null);
    setShowSettings(false);
    setSending(false);
    setSavingSettings(false);
    setResetting(false);
    setCheckins(agentId ? loadCheckins(agentId) : []);
  }, [agentId]);

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

  const isCurrentAgentLifecycle = useCallback(
    (operation: AgentOperation) =>
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

  const runCheckin = useCallback(
    async (targetAgentId: string, checkin: Checkin) => {
      const operation = beginAgentOperation(targetAgentId);
      const stamp = (patch: Partial<Checkin>) => {
        if (!isCurrentAgentLifecycle(operation)) return;
        setCheckins((current) => {
          if (!isCurrentAgentLifecycle(operation)) return current;
          const next = current.map((item) =>
            item.id === checkin.id ? { ...item, ...patch } : item,
          );
          saveCheckins(targetAgentId, next);
          return next;
        });
      };

      try {
        const { agent: updatedAgent, result } = await daemon.runAgent(
          targetAgentId,
          wrapPrompt(checkin),
          { kind: 'checkin', id: checkin.id },
        );
        adoptAgentSnapshot(operation, updatedAgent);
        const reply = result.data?.text?.trim() ?? '';
        if (result.status === 'error') {
          stamp({
            lastRunAtMs: Date.now(),
            lastOutcome: 'error',
            lastReply: result.error ?? 'run failed',
          });
        } else if (reply === CHECKIN_SENTINEL) {
          stamp({
            lastRunAtMs: Date.now(),
            lastOutcome: 'silent',
            lastReply: undefined,
          });
        } else {
          stamp({
            lastRunAtMs: Date.now(),
            lastOutcome: 'spoke',
            lastReply: reply,
          });
        }
      } catch (caught) {
        stamp({
          lastRunAtMs: Date.now(),
          lastOutcome: 'error',
          lastReply: caught instanceof Error ? caught.message : String(caught),
        });
      }
    },
    [adoptAgentSnapshot, beginAgentOperation, isCurrentAgentLifecycle],
  );

  useEffect(() => {
    if (!agentId) return;
    const timer = window.setInterval(async () => {
      if (
        checkinRunningRef.current ||
        sendingRef.current ||
        savingSettingsRef.current ||
        resetInFlightRef.current !== null
      ) {
        return;
      }
      const due = checkinsRef.current.filter((checkin) =>
        isDue(checkin, Date.now()),
      );
      if (due.length === 0) return;
      const runGeneration = ++checkinRunGenerationRef.current;
      const lifecycleGeneration = agentLifecycleGenerationRef.current;
      checkinRunningRef.current = true;
      try {
        for (const checkin of due) {
          if (
            runGeneration !== checkinRunGenerationRef.current ||
            lifecycleGeneration !== agentLifecycleGenerationRef.current ||
            resetInFlightRef.current !== null ||
            sendingRef.current ||
            savingSettingsRef.current
          ) {
            break;
          }
          await runCheckin(agentId, checkin);
        }
      } finally {
        if (runGeneration === checkinRunGenerationRef.current) {
          checkinRunningRef.current = false;
        }
      }
    }, 10_000);
    return () => {
      window.clearInterval(timer);
      checkinRunGenerationRef.current += 1;
      checkinRunningRef.current = false;
    };
  }, [agentId, runCheckin]);

  const addCheckin = () => {
    const text = ciPrompt.trim();
    if (!text || !agentId) return;
    setCheckins((current) => {
      const next = [
        ...current,
        newCheckin(text, Math.max(1, ciIntervalMin) * 60),
      ];
      saveCheckins(agentId, next);
      return next;
    });
    setCiPrompt('');
  };

  const removeCheckin = (id: string) => {
    if (!agentId) return;
    setCheckins((current) => {
      const next = current.filter((checkin) => checkin.id !== id);
      saveCheckins(agentId, next);
      return next;
    });
  };

  const toggleSettings = () => setShowSettings((visible) => !visible);

  const saveSettings = async (patch: {
    name?: string;
    model?: string;
    provider?: string;
    system?: string;
    tools?: string[];
  }): Promise<boolean> => {
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
    setError(null);
    try {
      const { agent: updatedAgent } = await daemon.updateAgent(agent.id, patch);
      return adoptAgentSnapshot(operation, updatedAgent);
    } catch (caught) {
      if (isCurrentAgentOperation(operation)) {
        setError(caught instanceof Error ? caught.message : String(caught));
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
    if (!agent || resetInFlightRef.current !== null) return;
    const targetAgentId = agent.id;
    const operation = beginAgentOperation(targetAgentId);
    resetInFlightRef.current = operation;
    setResetting(true);
    setError(null);
    try {
      try {
        await daemon.deleteAgent(targetAgentId);
      } catch (caught) {
        if (isCurrentResetOperation(operation)) {
          setError(caught instanceof Error ? caught.message : String(caught));
        }
        return;
      }

      const ownsSelectedMain = isCurrentResetOperation(operation);
      if (ownsSelectedMain) {
        agentLifecycleGenerationRef.current += 1;
        agentOperationGenerationRef.current += 1;
        currentAgentIdRef.current = null;
      }
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

  const send = async () => {
    const text = draft.trim();
    if (
      !text ||
      !agent ||
      sendingRef.current ||
      resetInFlightRef.current !== null
    ) {
      return;
    }
    const operation = beginAgentOperation(agent.id);
    sendingOperationGenerationRef.current = operation.generation;
    sendingRef.current = true;
    setSending(true);
    setError(null);
    setDraft('');
    try {
      const { agent: updatedAgent, result } = await daemon.runAgent(
        agent.id,
        text,
      );
      if (adoptAgentSnapshot(operation, updatedAgent)) {
        if (result.status === 'error') {
          setError(result.error ?? 'run failed');
        }
        scrollDown();
      }
    } catch (caught) {
      if (isCurrentAgentOperation(operation)) {
        setError(caught instanceof Error ? caught.message : String(caught));
      }
    } finally {
      if (sendingOperationGenerationRef.current === operation.generation) {
        sendingOperationGenerationRef.current = null;
        sendingRef.current = false;
        setSending(false);
      }
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
      <OnboardingFlow
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
          scrollDown();
        }}
      />
    );
  }

  const settingsPanel = showSettings ? (
    <SettingsPanel
      agent={agent}
      providers={providers}
      saving={savingSettings}
      resetting={resetting}
      error={error}
      saveSettings={saveSettings}
      resetAgent={resetAgent}
      close={toggleSettings}
    />
  ) : null;

  return (
    <>
      <WorkspaceShell
        key={agent.id}
        mainAgent={agent}
        agents={agents}
        connection={connection}
        onOpenSettings={toggleSettings}
        workspace={
          <section
            className="flex h-full min-h-0 flex-col"
            aria-label="Workspace"
          >
            <MessageList
              agent={agent}
              sending={sending}
              scrollerRef={scrollerRef}
              onSuggestion={setDraft}
            />
            <Composer
              agentName={agent.name}
              draft={draft}
              setDraft={setDraft}
              sending={sending}
              disabled={resetting}
              onSend={send}
              error={error}
              onDismissError={() => setError(null)}
            />
          </section>
        }
        activity={
          <ActivityView
            agent={agent}
            checkins={checkins}
            prompt={ciPrompt}
            setPrompt={setCiPrompt}
            intervalMin={ciIntervalMin}
            setIntervalMin={setCiIntervalMin}
            addCheckin={addCheckin}
            removeCheckin={removeCheckin}
            error={error}
          />
        }
      />
      {settingsPanel}
    </>
  );
}
