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
import { TelegramSettings } from './components/TelegramSettings';
import { TelegramThread } from './components/TelegramThread';
import { WorkspaceShell } from './components/WorkspaceShell';
import { useAgentIntegrations } from './hooks/useAgentIntegrations';
import { useDaemonBootstrap } from './hooks/useDaemonBootstrap';
import { clearCheckins, importLegacyCheckins } from './lib/checkins';
import {
  daemon,
  toAgentDetail,
  type AgentUpdateInput,
  type DaemonSnapshot,
} from './lib/daemon-api';
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
            className="rounded-xl bg-accent px-4 py-2 text-sm font-semibold text-abyss shadow-lg shadow-accent/20 transition hover:bg-accent/90"
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
  const agent = selectMainAgent(agents);

  const [draft, setDraft] = useState('');
  const [sending, setSending] = useState(false);
  const [workspaceError, setWorkspaceError] = useState<string | null>(null);
  const [settingsSaveError, setSettingsSaveError] = useState<string | null>(
    null,
  );
  const [resetError, setResetError] = useState<string | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  const [savingSettings, setSavingSettings] = useState(false);
  const [resetting, setResetting] = useState(false);
  const [ciPrompt, setCiPrompt] = useState('');
  const [ciIntervalMin, setCiIntervalMin] = useState(30);
  const [ciTarget, setCiTarget] = useState<'workspace' | 'telegram'>(
    'workspace',
  );
  const [legacyMigrationError, setLegacyMigrationError] = useState<
    string | null
  >(null);

  const sendingRef = useRef(false);
  const savingSettingsRef = useRef(false);
  const agentOperationGenerationRef = useRef(0);
  const agentLifecycleGenerationRef = useRef(0);
  const sendingOperationGenerationRef = useRef<number | null>(null);
  const settingsOperationGenerationRef = useRef<number | null>(null);
  const resetInFlightRef = useRef<AgentOperation | null>(null);
  const settingsTriggerRef = useRef<HTMLElement | null>(null);
  const currentAgentIdRef = useRef<string | null>(null);
  const previousSelectedMainIdRef = useRef<string | null>(null);
  sendingRef.current = sending;

  const agentId = agent?.id ?? null;
  const integrations = useAgentIntegrations(agentId);
  const telegramConnector = integrations.connectors[0] ?? null;
  useLayoutEffect(() => {
    if (previousSelectedMainIdRef.current === agentId) return;

    previousSelectedMainIdRef.current = agentId;
    agentLifecycleGenerationRef.current += 1;
    agentOperationGenerationRef.current += 1;
    currentAgentIdRef.current = agentId;
    sendingOperationGenerationRef.current = null;
    settingsOperationGenerationRef.current = null;
    resetInFlightRef.current = null;
    settingsTriggerRef.current = null;
    sendingRef.current = false;
    savingSettingsRef.current = false;

    setDraft('');
    setCiPrompt('');
    setCiIntervalMin(30);
    setCiTarget('workspace');
    setLegacyMigrationError(null);
    setWorkspaceError(null);
    setSettingsSaveError(null);
    setResetError(null);
    setShowSettings(false);
    setSending(false);
    setSavingSettings(false);
    setResetting(false);
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

  useEffect(() => {
    if (telegramConnector) void integrations.loadMessages(telegramConnector.id);
  }, [telegramConnector?.id]);

  const addCheckin = async () => {
    const text = ciPrompt.trim();
    if (!text || !agentId) return;
    const added = await integrations.createSchedule({
      prompt: text,
      trigger: {
        type: 'interval',
        intervalMs: Math.max(1, ciIntervalMin) * 60_000,
      },
      target:
        ciTarget === 'telegram' && telegramConnector
          ? { type: 'connector', connectorId: telegramConnector.id }
          : { type: 'workspace' },
    });
    if (added) setCiPrompt('');
  };

  const removeCheckin = (id: string) => {
    void integrations.removeSchedule(id);
  };

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
        setSettingsSaveError(
          caught instanceof Error ? caught.message : String(caught),
        );
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
          setResetError(
            caught instanceof Error ? caught.message : String(caught),
          );
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
    setWorkspaceError(null);
    setDraft('');
    try {
      const { agent: updatedAgent, result } = await daemon.runAgent(
        agent.id,
        text,
      );
      if (adoptAgentSnapshot(operation, updatedAgent)) {
        if (result.status === 'error') {
          setWorkspaceError(result.error ?? 'run failed');
        }
        scrollDown();
      }
    } catch (caught) {
      if (isCurrentAgentOperation(operation)) {
        setWorkspaceError(
          caught instanceof Error ? caught.message : String(caught),
        );
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
          void refreshWorkspace();
          scrollDown();
        }}
      />
    );
  }

  const settingsPanel = showSettings ? (
    <SettingsPanel
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
      integrations={
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
  ) : null;

  return (
    <>
      <div
        data-testid="workspace-background"
        className="contents"
        aria-hidden={showSettings || undefined}
        inert={showSettings || undefined}
      >
        <WorkspaceShell
          key={agent.id}
          mainAgent={agent}
          agents={agents}
          connection={connection}
          onOpenSettings={openSettings}
          onChangeWorkspaceAvatar={changeWorkspaceAvatar}
          workspaceState={workspace}
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
                error={workspaceError}
                onDismissError={() => setWorkspaceError(null)}
              />
            </section>
          }
          activity={
            <ActivityView
              agent={agent}
              checkins={integrations.schedules}
              prompt={ciPrompt}
              setPrompt={setCiPrompt}
              intervalMin={ciIntervalMin}
              setIntervalMin={setCiIntervalMin}
              addCheckin={addCheckin}
              removeCheckin={removeCheckin}
              error={integrations.scheduleError ?? legacyMigrationError}
              target={ciTarget}
              setTarget={setCiTarget}
              telegramAvailable={telegramConnector?.approvedChat != null}
              busy={integrations.scheduleBusy}
            />
          }
          telegram={
            telegramConnector ? (
              <TelegramThread
                agentName={agent.name}
                messages={integrations.messages}
                hasOlder={integrations.nextBefore !== null}
                busy={integrations.connectorBusy}
                error={integrations.connectorError}
                deliveryQueued={integrations.deliveryQueued}
                loadOlder={() =>
                  integrations.loadMessages(telegramConnector.id, true)
                }
                send={(text) =>
                  integrations.sendTelegramMessage(telegramConnector.id, text)
                }
              />
            ) : null
          }
        />
      </div>
      {settingsPanel}
    </>
  );
}
