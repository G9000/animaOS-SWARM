import { useCallback, useEffect, useRef, useState } from 'react';
import {
  daemon,
  toAgentDetail,
  MODEL_SUGGESTIONS,
  type DaemonProvider,
} from './lib/daemon-api';
import type { AgentDetail } from './lib/types';
import { toolNamesForProfile } from './lib/agent-access';
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
import { SetupScreen } from './components/SetupScreen';
import { ChatHeader, Composer, MessageList } from './components/ChatScreen';
import { SettingsPanel } from './components/SettingsPanel';
import { CheckinsView } from './components/CheckinsView';
import { Sidebar } from './components/Sidebar';

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
  const [name, setName] = useState('');
  const [provider, setProvider] = useState<string>('anthropic');
  const [model, setModel] = useState(MODEL_SUGGESTIONS['anthropic'][0]);
  const [customModel, setCustomModel] = useState('');
  const [system, setSystem] = useState('');
  const [creating, setCreating] = useState(false);

  // Settings panel state
  const [showSettings, setShowSettings] = useState(false);
  const [savingSettings, setSavingSettings] = useState(false);

  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  const [view, setView] = useState<'chat' | 'checkins'>('chat');

  // Check-ins (proactive recurring prompts, scheduled from this tab)
  const [checkins, setCheckins] = useState<Checkin[]>([]);
  const [ciPrompt, setCiPrompt] = useState('');
  const [ciIntervalMin, setCiIntervalMin] = useState(30);

  const resolvedModel = model === '__custom__' ? customModel.trim() : model;
  const sendingRef = useRef(false);
  sendingRef.current = sending;

  const scrollerRef = useRef<HTMLDivElement>(null);
  const scrollDown = () => {
    requestAnimationFrame(() => {
      const el = scrollerRef.current;
      if (el) el.scrollTop = el.scrollHeight;
    });
  };

  const refreshAgent = useCallback(async () => {
    try {
      const { agents } = await daemon.listAgents();
      setAgent(agents.length > 0 ? toAgentDetail(agents[0]) : null);
      setOnline(true);
      setLoaded(true);
      if (agents.length > 0) scrollDown();
    } catch {
      setOnline(false);
      setLoaded(true);
    }
  }, []);

  useEffect(() => { refreshAgent(); }, [refreshAgent]);

  // Load daemon provider catalog (which providers have keys configured).
  useEffect(() => {
    daemon.listProviders()
      .then(({ providers }) => setProviders(providers))
      .catch(() => setProviders(null));
  }, []);

  // Light polling so new messages appear while idle.
  useEffect(() => {
    const timer = setInterval(() => {
      if (!sendingRef.current) {
        daemon.listAgents()
          .then(({ agents }) => {
            setAgent(agents.length > 0 ? toAgentDetail(agents[0]) : null);
            setOnline(true);
            if (agents.length > 0) scrollDown();
          })
          .catch(() => setOnline(false));
      }
    }, 5000);
    return () => clearInterval(timer);
  }, []);

  // ── Check-ins: local scheduler firing recurring prompts through the daemon ──

  const agentId = agent?.id ?? null;
  const checkinsRef = useRef<Checkin[]>([]);
  checkinsRef.current = checkins;
  const checkinRunningRef = useRef(false);

  // Load this agent's check-ins whenever the agent changes.
  useEffect(() => {
    setCheckins(agentId ? loadCheckins(agentId) : []);
  }, [agentId]);

  const runCheckin = useCallback(async (targetAgentId: string, c: Checkin) => {
    const stamp = (patch: Partial<Checkin>) =>
      setCheckins((prev) => {
        const next = prev.map((x) => (x.id === c.id ? { ...x, ...patch } : x));
        saveCheckins(targetAgentId, next);
        return next;
      });
    try {
      const { result } = await daemon.runAgent(targetAgentId, wrapPrompt(c), {
        kind: 'checkin',
        id: c.id,
      });
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
  }, []);

  // 10s ticker: fire whichever check-ins are due, one at a time.
  useEffect(() => {
    if (!agentId) return;
    const timer = setInterval(async () => {
      if (checkinRunningRef.current || sendingRef.current) return;
      const due = checkinsRef.current.filter((c) => isDue(c, Date.now()));
      if (due.length === 0) return;
      checkinRunningRef.current = true;
      try {
        for (const c of due) {
          await runCheckin(agentId, c);
        }
        await refreshAgent();
      } finally {
        checkinRunningRef.current = false;
      }
    }, 10_000);
    return () => clearInterval(timer);
  }, [agentId, runCheckin, refreshAgent]);

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

  const createAgent = async () => {
    if (!resolvedModel) { setError('model is required'); return; }
    setCreating(true);
    setError(null);
    try {
      await daemon.createAgent({
        name: name.trim() || resolvedModel,
        provider,
        model: resolvedModel,
        system: system.trim() || undefined,
        tools: toolNamesForProfile('collaborate'),
      });
      setName('');
      setCustomModel('');
      setSystem('');
      await refreshAgent();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setCreating(false);
    }
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
    setSavingSettings(true);
    setError(null);
    try {
      await daemon.updateAgent(agent.id, patch);
      await refreshAgent();
      return true;
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      return false;
    } finally {
      setSavingSettings(false);
    }
  };

  const resetAgent = async () => {
    if (!agent) return;
    setError(null);
    try {
      await daemon.deleteAgent(agent.id);
      clearCheckins(agent.id);
      setCheckins([]);
      setAgent(null);
      setShowSettings(false);
      setView('chat');
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const send = async () => {
    const text = draft.trim();
    if (!text || !agent || sending) return;
    setSending(true);
    setError(null);
    setDraft('');
    try {
      const { result } = await daemon.runAgent(agent.id, text);
      if (result.status === 'error') setError(result.error ?? 'run failed');
      await refreshAgent();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSending(false);
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
          /* ── Setup screen: no agent yet ── */
          <SetupScreen
            providers={providers}
            name={name}
            setName={setName}
            provider={provider}
            setProvider={setProvider}
            model={model}
            setModel={setModel}
            customModel={customModel}
            setCustomModel={setCustomModel}
            system={system}
            setSystem={setSystem}
            creating={creating}
            error={error}
            online={online}
            createAgent={createAgent}
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
