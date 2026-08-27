import { useCallback, useEffect, useRef, useState } from 'react';
import {
  daemon,
  toAgentDetail,
  type DaemonProvider,
  type DaemonSnapshot,
} from './lib/daemon-api';
import type { AgentDetail } from './lib/types';
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
import { OnboardingFlow } from './components/onboarding/OnboardingFlow';
import { ChatHeader, Composer, MessageList } from './components/ChatScreen';
import { SettingsPanel } from './components/SettingsPanel';
import { CheckinsView } from './components/CheckinsView';
import { Sidebar } from './components/Sidebar';

interface AgentOperation {
  generation: number;
  lifecycleGeneration: number;
  targetAgentId: string;
}

/**
 * Single-agent console on top of the anima-daemon.
 * The daemon is multi-agent; this UI intentionally drives only the first
 * agent so the single-agent loop (create → chat → reset) can be validated
 * end to end before multi-agent UX is built.
 */
export function ViewHarness() {
  const [online, setOnline] = useState<boolean | null>(null);
  const [agent, setAgent] = useState<AgentDetail | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [draft, setDraft] = useState('');
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [providers, setProviders] = useState<DaemonProvider[] | null>(null);
  const [providersError, setProvidersError] = useState<string | null>(null);

  // Settings panel state
  const [showSettings, setShowSettings] = useState(false);
  const [savingSettings, setSavingSettings] = useState(false);

  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  const [view, setView] = useState<'chat' | 'checkins'>('chat');

  // Check-ins (proactive recurring prompts, scheduled from this tab)
  const [checkins, setCheckins] = useState<Checkin[]>([]);
  const [ciPrompt, setCiPrompt] = useState('');
  const [ciIntervalMin, setCiIntervalMin] = useState(30);

  const sendingRef = useRef(false);
  const agentMutationEpochRef = useRef(0);
  const agentOperationGenerationRef = useRef(0);
  const agentLifecycleGenerationRef = useRef(0);
  const sendingOperationGenerationRef = useRef<number | null>(null);
  const settingsOperationGenerationRef = useRef<number | null>(null);
  const currentAgentIdRef = useRef<string | null>(null);
  const agentRequestTokenRef = useRef(0);
  const agentRequestInFlightRef = useRef<Promise<void> | null>(null);
  const providerRequestGenerationRef = useRef(0);
  sendingRef.current = sending;

  const beginAgentOperation = useCallback(
    (targetAgentId: string): AgentOperation => {
      agentMutationEpochRef.current += 1;
      return {
        generation: ++agentOperationGenerationRef.current,
        lifecycleGeneration: agentLifecycleGenerationRef.current,
        targetAgentId,
      };
    },
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

  const adoptAgentSnapshot = useCallback(
    (operation: AgentOperation, snapshot: DaemonSnapshot) => {
      if (
        !isCurrentAgentOperation(operation) ||
        snapshot.state.id !== operation.targetAgentId
      ) {
        return false;
      }

      const updatedAgent = toAgentDetail(snapshot);
      agentMutationEpochRef.current += 1;
      currentAgentIdRef.current = updatedAgent.id;
      setAgent(updatedAgent);
      setOnline(true);
      setLoaded(true);
      return true;
    },
    [isCurrentAgentOperation],
  );

  const scrollerRef = useRef<HTMLDivElement>(null);
  const scrollDown = () => {
    requestAnimationFrame(() => {
      const el = scrollerRef.current;
      if (el) el.scrollTop = el.scrollHeight;
    });
  };

  const refreshAgent = useCallback(() => {
    if (agentRequestInFlightRef.current) {
      return agentRequestInFlightRef.current;
    }

    const mutationEpoch = agentMutationEpochRef.current;
    const requestToken = ++agentRequestTokenRef.current;
    const request = (async () => {
      try {
        const { agents } = await daemon.listAgents();
        if (
          requestToken === agentRequestTokenRef.current &&
          mutationEpoch === agentMutationEpochRef.current
        ) {
          const refreshedAgent =
            agents.length > 0 ? toAgentDetail(agents[0]) : null;
          if (refreshedAgent?.id !== currentAgentIdRef.current) {
            agentLifecycleGenerationRef.current += 1;
          }
          currentAgentIdRef.current = refreshedAgent?.id ?? null;
          setAgent(refreshedAgent);
          setOnline(true);
          setLoaded(true);
          if (agents.length > 0) {
            scrollDown();
          }
        }
      } catch {
        if (
          requestToken === agentRequestTokenRef.current &&
          mutationEpoch === agentMutationEpochRef.current
        ) {
          setOnline(false);
          setLoaded(true);
        }
      }
    })();

    const ownedRequest = request.finally(() => {
      if (agentRequestInFlightRef.current === ownedRequest) {
        agentRequestInFlightRef.current = null;
      }
    });
    agentRequestInFlightRef.current = ownedRequest;
    return ownedRequest;
  }, []);

  useEffect(() => {
    void refreshAgent();
  }, [refreshAgent]);

  const retryProviders = useCallback(async () => {
    const requestGeneration = ++providerRequestGenerationRef.current;
    try {
      const response = await daemon.listProviders();
      if (requestGeneration === providerRequestGenerationRef.current) {
        setProviders(response.providers);
        setProvidersError(null);
      }
    } catch (providerError) {
      if (requestGeneration === providerRequestGenerationRef.current) {
        setProvidersError(
          providerError instanceof Error
            ? providerError.message
            : String(providerError),
        );
      }
    }
  }, []);

  // Load daemon provider catalog (which providers have keys configured).
  useEffect(() => {
    void retryProviders();
    return () => {
      providerRequestGenerationRef.current += 1;
    };
  }, [retryProviders]);

  // Light polling so new messages appear while idle.
  useEffect(() => {
    const timer = setInterval(() => {
      if (!sendingRef.current && !agentRequestInFlightRef.current) {
        void refreshAgent();
      }
    }, 5000);
    return () => clearInterval(timer);
  }, [refreshAgent]);

  // ── Check-ins: local scheduler firing recurring prompts through the daemon ──

  const agentId = agent?.id ?? null;
  const checkinsRef = useRef<Checkin[]>([]);
  checkinsRef.current = checkins;
  const checkinRunningRef = useRef(false);
  const checkinRunGenerationRef = useRef(0);

  // Load this agent's check-ins whenever the agent changes.
  useEffect(() => {
    setCheckins(agentId ? loadCheckins(agentId) : []);
  }, [agentId]);

  const runCheckin = useCallback(async (targetAgentId: string, c: Checkin) => {
    const operation = beginAgentOperation(targetAgentId);
    const stamp = (patch: Partial<Checkin>) => {
      if (!isCurrentAgentLifecycle(operation)) return;
      setCheckins((prev) => {
        if (!isCurrentAgentLifecycle(operation)) return prev;
        const next = prev.map((x) => (x.id === c.id ? { ...x, ...patch } : x));
        saveCheckins(targetAgentId, next);
        return next;
      });
    };
    try {
      const { agent: updatedAgent, result } = await daemon.runAgent(
        targetAgentId,
        wrapPrompt(c),
        {
          kind: 'checkin',
          id: c.id,
        },
      );
      adoptAgentSnapshot(operation, updatedAgent);
      const reply = result.data?.text?.trim() ?? '';
      if (result.status === 'error') {
        stamp({ lastRunAtMs: Date.now(), lastOutcome: 'error', lastReply: result.error ?? 'run failed' });
      } else if (reply === CHECKIN_SENTINEL) {
        stamp({ lastRunAtMs: Date.now(), lastOutcome: 'silent', lastReply: undefined });
      } else {
        stamp({ lastRunAtMs: Date.now(), lastOutcome: 'spoke', lastReply: reply });
      }
    } catch (e) {
      stamp({
        lastRunAtMs: Date.now(),
        lastOutcome: 'error',
        lastReply: e instanceof Error ? e.message : String(e),
      });
    }
  }, [adoptAgentSnapshot, beginAgentOperation, isCurrentAgentLifecycle]);

  // 10s ticker: fire whichever check-ins are due, one at a time.
  useEffect(() => {
    if (!agentId) return;
    const timer = setInterval(async () => {
      if (checkinRunningRef.current || sendingRef.current) return;
      const due = checkinsRef.current.filter((c) => isDue(c, Date.now()));
      if (due.length === 0) return;
      const runGeneration = ++checkinRunGenerationRef.current;
      checkinRunningRef.current = true;
      try {
        for (const c of due) {
          await runCheckin(agentId, c);
        }
      } finally {
        if (runGeneration === checkinRunGenerationRef.current) {
          checkinRunningRef.current = false;
        }
      }
    }, 10_000);
    return () => {
      clearInterval(timer);
      checkinRunGenerationRef.current += 1;
      checkinRunningRef.current = false;
    };
  }, [agentId, runCheckin]);

  const addCheckin = () => {
    const text = ciPrompt.trim();
    if (!text || !agentId) return;
    setCheckins((prev) => {
      const next = [...prev, newCheckin(text, Math.max(1, ciIntervalMin) * 60)];
      saveCheckins(agentId, next);
      return next;
    });
    setCiPrompt('');
  };

  const removeCheckin = (id: string) => {
    if (!agentId) return;
    setCheckins((prev) => {
      const next = prev.filter((c) => c.id !== id);
      saveCheckins(agentId, next);
      return next;
    });
  };

  const toggleSettings = () => setShowSettings((v) => !v);

  /** PATCH the daemon config in place; conversation is preserved. */
  const saveSettings = async (patch: {
    name?: string;
    model?: string;
    provider?: string;
    system?: string;
  }): Promise<boolean> => {
    if (!agent) return false;
    const operation = beginAgentOperation(agent.id);
    settingsOperationGenerationRef.current = operation.generation;
    setSavingSettings(true);
    setError(null);
    try {
      const { agent: updatedAgent } = await daemon.updateAgent(agent.id, patch);
      return adoptAgentSnapshot(operation, updatedAgent);
    } catch (e) {
      if (isCurrentAgentOperation(operation)) {
        setError(e instanceof Error ? e.message : String(e));
      }
      return false;
    } finally {
      if (settingsOperationGenerationRef.current === operation.generation) {
        settingsOperationGenerationRef.current = null;
        setSavingSettings(false);
      }
    }
  };

  const resetAgent = async () => {
    if (!agent) return;
    const targetAgentId = agent.id;
    const operation = beginAgentOperation(targetAgentId);
    const sendingGeneration = sendingOperationGenerationRef.current;
    const settingsGeneration = settingsOperationGenerationRef.current;
    setError(null);
    try {
      await daemon.deleteAgent(targetAgentId);
      agentMutationEpochRef.current += 1;
      agentLifecycleGenerationRef.current += 1;
      clearCheckins(targetAgentId);
      if (currentAgentIdRef.current === targetAgentId) {
        if (sendingOperationGenerationRef.current === sendingGeneration) {
          sendingOperationGenerationRef.current = null;
          sendingRef.current = false;
          setSending(false);
        }
        if (settingsOperationGenerationRef.current === settingsGeneration) {
          settingsOperationGenerationRef.current = null;
          setSavingSettings(false);
        }
        currentAgentIdRef.current = null;
        setCheckins([]);
        setAgent(null);
        setShowSettings(false);
        setView('chat');
      }
    } catch (e) {
      if (isCurrentAgentOperation(operation)) {
        setError(e instanceof Error ? e.message : String(e));
      }
    }
  };

  const send = async () => {
    const text = draft.trim();
    if (!text || !agent || sending) return;
    const operation = beginAgentOperation(agent.id);
    sendingOperationGenerationRef.current = operation.generation;
    setSending(true);
    setError(null);
    setDraft('');
    try {
      const { agent: updatedAgent, result } = await daemon.runAgent(agent.id, text);
      if (adoptAgentSnapshot(operation, updatedAgent)) {
        if (result.status === 'error') setError(result.error ?? 'run failed');
        scrollDown();
      }
    } catch (e) {
      if (isCurrentAgentOperation(operation)) {
        setError(e instanceof Error ? e.message : String(e));
      }
    } finally {
      if (sendingOperationGenerationRef.current === operation.generation) {
        sendingOperationGenerationRef.current = null;
        setSending(false);
      }
    }
  };

  const openSettings = () => {
    if (agent) toggleSettings();
  };

  const settingsPanel = showSettings && agent && (
    <SettingsPanel
      agent={agent}
      providers={providers}
      saving={savingSettings}
      error={error}
      saveSettings={saveSettings}
      resetAgent={resetAgent}
      close={toggleSettings}
    />
  );

  return (
    <div className="relative z-[1] flex min-h-0 flex-1">
      <Sidebar
        agent={agent}
        online={online}
        collapsed={sidebarCollapsed}
        activeView={view}
        checkinCount={checkins.length}
        onNavigate={setView}
        onToggleCollapse={() => setSidebarCollapsed((v) => !v)}
        onOpenSettings={openSettings}
      />

      <main className="relative flex min-w-0 flex-1 flex-col">
        {!loaded ? (
          <div className="flex flex-1 items-center justify-center">
            <div className="flex flex-col items-center gap-3">
              <div className="flex items-center gap-1.5">
                {[0, 1, 2].map((i) => (
                  <span
                    key={i}
                    className="typing-dot h-2 w-2 rounded-full bg-sky-400"
                    style={{ animationDelay: `${i * 150}ms` }}
                  />
                ))}
              </div>
              <span className="font-mono text-xs text-ink-3">connecting to daemon…</span>
            </div>
          </div>
        ) : !agent ? (
          /* ── Guided onboarding: no agent yet ── */
          <OnboardingFlow
            providers={providers}
            providersError={providersError}
            retryProviders={retryProviders}
            onCreated={(snapshot) => {
              agentMutationEpochRef.current += 1;
              agentOperationGenerationRef.current += 1;
              agentLifecycleGenerationRef.current += 1;
              const createdAgent = toAgentDetail(snapshot);
              currentAgentIdRef.current = createdAgent.id;
              setAgent(createdAgent);
              setOnline(true);
              scrollDown();
            }}
          />
        ) : view === 'checkins' ? (
          /* ── Check-ins view ── */
          <>
            <CheckinsView
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
            {settingsPanel}
          </>
        ) : (
          /* ── Chat screen ── */
          <>
            <ChatHeader agent={agent} online={online} onOpenSettings={toggleSettings} />
            {settingsPanel}

            <MessageList
              agent={agent}
              sending={sending}
              scrollerRef={scrollerRef}
              onSuggestion={(text) => setDraft(text)}
            />

            <Composer
              agentName={agent.name}
              draft={draft}
              setDraft={setDraft}
              sending={sending}
              onSend={send}
              error={error}
              onDismissError={() => setError(null)}
            />
          </>
        )}
      </main>
    </div>
  );
}
