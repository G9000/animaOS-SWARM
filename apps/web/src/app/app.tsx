import { useEffect, useMemo, useRef, useState } from 'react';
import {
  agents, health, memories, providers as providerApi, swarms,
  type AgentSnapshot, type HealthResponse, type Memory,
  type ProviderResponse, type SwarmState, type SwarmStreamEvent, type TaskResult,
} from '../lib/api';
import { getColors, DARK, MONO, SANS } from '../design';
import { NewAgentModal, NewSwarmModal, RunModal } from '../Modals';
import { ViewHome }         from '../ViewHome';
import { ViewFleet }        from '../ViewFleet';
import { ViewSwarms }       from '../ViewSwarms';
import { ViewMessages }     from '../ViewMessages';
import { ViewMemory }       from '../ViewMemory';
import { ViewAgentDetail }  from '../ViewAgentDetail';

// ── Types ─────────────────────────────────────────────────────────────────────
type View =
  | { name: 'overview' }
  | { name: 'fleet' }
  | { name: 'swarms' }
  | { name: 'messages' }
  | { name: 'memory' }
  | { name: 'agent'; id: string };

interface RunResult { status: string; durationMs?: number; output: string | null; error: string | null; }

function formatOutput(data: unknown): string | null {
  if (data == null) return null;
  if (typeof data === 'string') return data;
  if (typeof data === 'object' && 'text' in (data as object)) return String((data as { text: unknown }).text);
  return JSON.stringify(data, null, 2);
}

// ── Login ─────────────────────────────────────────────────────────────────────
function LoginPage({ onEnter }: { onEnter: () => void }) {
  const [email, setEmail] = useState('');
  const [pass, setPass]   = useState('');
  const c = DARK;
  const IS: React.CSSProperties = {
    width: '100%', padding: '10px 12px', fontSize: 14,
    background: 'rgba(255,255,255,0.06)', color: c.textPrimary,
    border: `1px solid ${c.border}`, outline: 'none', fontFamily: 'inherit',
  };

  return (
    <div style={{ display: 'flex', minHeight: '100vh', background: c.bg, color: c.textPrimary, fontFamily: SANS }}>
      {/* Brand panel */}
      <div style={{ display: 'none', width: '52%', flexDirection: 'column', justifyContent: 'space-between', padding: '48px 56px', background: '#080a0e', ...(window.innerWidth >= 1024 ? { display: 'flex' } : {}) }}>
        <span style={{ fontSize: 11, fontFamily: MONO, letterSpacing: 2, textTransform: 'uppercase', color: 'rgba(255,255,255,0.4)', border: '1px solid rgba(255,255,255,0.08)', padding: '6px 12px', display: 'inline-block' }}>
          animaOS
        </span>
        <div>
          <h1 style={{ fontSize: 40, fontWeight: 700, lineHeight: 1.2, margin: '0 0 16px', letterSpacing: -1 }}>
            The runtime for<br />autonomous agents.
          </h1>
          <p style={{ fontSize: 15, color: 'rgba(255,255,255,0.45)', margin: '0 0 40px', lineHeight: 1.65 }}>
            Manage swarms, memory, and live inference from one control room.
          </p>
          {[
            { icon: '⬡', text: 'Multi-agent swarms — supervisor, dynamic, or round-robin' },
            { icon: '◈', text: 'Persistent vector memory — search, score, and replay context' },
            { icon: '◎', text: 'Live SSE event streaming with real-time execution tracing' },
          ].map(({ icon, text }) => (
            <div key={text} style={{ display: 'flex', gap: 12, marginBottom: 14, fontSize: 14, color: 'rgba(255,255,255,0.45)', alignItems: 'flex-start' }}>
              <span style={{ color: '#38bdf8', flexShrink: 0, marginTop: 1 }}>{icon}</span>
              {text}
            </div>
          ))}
        </div>
        <p style={{ fontSize: 11, color: 'rgba(255,255,255,0.18)', fontFamily: MONO }}>animaOS · runtime console</p>
      </div>

      {/* Form panel */}
      <div style={{ flex: 1, display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center', padding: '48px 24px' }}>
        <div style={{ width: '100%', maxWidth: 360 }}>
          <h2 style={{ fontSize: 24, fontWeight: 700, margin: '0 0 6px' }}>Sign in</h2>
          <p style={{ fontSize: 14, color: c.textMuted, margin: '0 0 32px' }}>Access the control room.</p>

          <form onSubmit={e => { e.preventDefault(); onEnter(); }} style={{ display: 'flex', flexDirection: 'column', gap: 14 }}>
            <div style={{ display: 'flex', flexDirection: 'column', gap: 5 }}>
              <label style={{ fontSize: 9, fontFamily: MONO, letterSpacing: 1.4, textTransform: 'uppercase', color: c.textMuted }}>Email</label>
              <input type="email" style={IS} value={email} onChange={e => setEmail(e.target.value)} placeholder="you@example.com" autoComplete="email" />
            </div>
            <div style={{ display: 'flex', flexDirection: 'column', gap: 5 }}>
              <label style={{ fontSize: 9, fontFamily: MONO, letterSpacing: 1.4, textTransform: 'uppercase', color: c.textMuted }}>Password</label>
              <input type="password" style={IS} value={pass} onChange={e => setPass(e.target.value)} placeholder="••••••••" autoComplete="current-password" />
            </div>
            <button type="submit" style={{ marginTop: 6, padding: '12px 0', fontSize: 14, fontWeight: 600, color: '#fff', background: c.accent, border: 'none', cursor: 'pointer' }}>
              Sign in
            </button>
          </form>

          <div style={{ margin: '24px 0', display: 'flex', alignItems: 'center', gap: 12 }}>
            <div style={{ flex: 1, height: 1, background: c.border }} />
            <span style={{ fontSize: 12, color: c.textMuted }}>or</span>
            <div style={{ flex: 1, height: 1, background: c.border }} />
          </div>

          <button type="button" onClick={onEnter}
            style={{ width: '100%', padding: '11px 0', fontSize: 13, background: 'transparent', border: `1px solid ${c.border}`, color: c.textMuted, cursor: 'pointer' }}>
            Continue without signing in
          </button>
          <p style={{ textAlign: 'center', fontSize: 11, color: c.textMuted, marginTop: 12, fontFamily: MONO }}>
            Dev bypass · no credentials needed locally
          </p>
        </div>
      </div>
    </div>
  );
}

// ── App root ──────────────────────────────────────────────────────────────────
export function App() {
  const [loggedIn, setLoggedIn] = useState(false);
  if (!loggedIn) return <LoginPage onEnter={() => setLoggedIn(true)} />;
  return <Dashboard onSignOut={() => setLoggedIn(false)} />;
}

// ── Dashboard ─────────────────────────────────────────────────────────────────
function Dashboard({ onSignOut }: { onSignOut: () => void }) {
  const [dark, setDark]   = useState(true);
  const c = getColors(dark);

  // Navigation
  const [view, setView]   = useState<View>({ name: 'overview' });
  const navigate = (name: string, params?: Record<string, string>) => {
    if (name === 'agent' && params?.id) setView({ name: 'agent', id: params.id });
    else setView({ name: name as View['name'] } as View);
  };

  // Data
  const [healthState, setHealthState]   = useState<HealthResponse | null>(null);
  const [providerList, setProviderList] = useState<ProviderResponse[]>([]);
  const [agentList, setAgentList]       = useState<AgentSnapshot[]>([]);
  const [swarmList, setSwarmList]       = useState<SwarmState[]>([]);
  const [recentMems, setRecentMems]     = useState<Memory[]>([]);
  const [loading, setLoading]           = useState(true);
  const [globalError, setGlobalError]   = useState<string | null>(null);
  const [refreshNonce, setRefreshNonce] = useState(0);
  const [tick, setTick]                 = useState(0);

  // Swarm streaming
  const [streamingId, setStreamingId]   = useState<string | null>(null);
  const [liveEvents, setLiveEvents]     = useState<SwarmStreamEvent[]>([]);
  const streamRef = useRef<(() => void) | null>(null);

  // Modals
  const [showNewAgent, setShowNewAgent] = useState(false);
  const [showNewSwarm, setShowNewSwarm] = useState(false);
  const [runAgentTarget, setRunAgentTarget] = useState<AgentSnapshot | null>(null);
  const [runSwarmTarget, setRunSwarmTarget] = useState<SwarmState | null>(null);
  const [deletingId, setDeletingId]     = useState<string | null>(null);

  // Tick for animated charts
  useEffect(() => {
    const t = setInterval(() => setTick(n => n + 1), 2000);
    return () => clearInterval(t);
  }, []);

  // Data polling
  useEffect(() => {
    let active = true;
    const load = async (bg = false) => {
      if (!bg) setLoading(true);
      try {
        const [h, p, a, sw, mem] = await Promise.all([
          health.get(), providerApi.list(), agents.list(), swarms.list(),
          memories.recent({ limit: 24 }),
        ]);
        if (!active) return;
        setHealthState(h); setProviderList(p); setAgentList(a);
        setSwarmList(sw); setRecentMems(mem); setGlobalError(null);
      } catch (err) {
        if (active) setGlobalError(err instanceof Error ? err.message : String(err));
      } finally { if (active) setLoading(false); }
    };
    void load();
    const id = setInterval(() => void load(true), 10_000);
    return () => { active = false; clearInterval(id); };
  }, [refreshNonce]);

  useEffect(() => () => { streamRef.current?.(); }, []);

  const configuredProviders = useMemo(() => providerList.filter(p => p.configured), [providerList]);
  const refresh = () => setRefreshNonce(n => n + 1);
  const daemonOk = healthState?.status === 'ok';

  function toggleStream(id: string) {
    if (streamingId === id) {
      streamRef.current?.(); streamRef.current = null;
      setStreamingId(null); setLiveEvents([]); return;
    }
    streamRef.current?.(); setLiveEvents([]); setStreamingId(id);
    streamRef.current = swarms.streamEvents(id,
      ev => setLiveEvents(prev => [ev, ...prev].slice(0, 60)),
      () => { setStreamingId(null); streamRef.current = null; }
    );
  }

  async function handleDeleteAgent(id: string) {
    setDeletingId(id);
    try { await agents.delete(id); refresh(); }
    catch (err) { setGlobalError(err instanceof Error ? err.message : String(err)); }
    finally { setDeletingId(null); }
  }

  async function runAgent(id: string, task: string): Promise<RunResult> {
    const r = await agents.run(id, task); refresh();
    return { status: r.result.status, durationMs: r.result.durationMs, output: formatOutput(r.result.data), error: r.result.error ?? null };
  }

  async function runSwarm(id: string, task: string): Promise<RunResult> {
    const r = await swarms.run(id, task); refresh();
    return { status: r.result.status, durationMs: r.result.durationMs, output: formatOutput(r.result.data), error: r.result.error ?? null };
  }

  const NAV: { id: View['name']; label: string; count?: number }[] = [
    { id: 'overview',  label: 'Overview' },
    { id: 'fleet',     label: 'Fleet',     count: agentList.length },
    { id: 'swarms',    label: 'Swarms',    count: swarmList.length },
    { id: 'messages',  label: 'Transcript' },
    { id: 'memory',    label: 'Memory',    count: recentMems.length },
  ];

  return (
    <div style={{ display: 'flex', height: '100vh', background: c.bg, color: c.textPrimary, fontFamily: SANS, fontSize: 13 }}>
      {/* ── Sidebar ── */}
      <aside style={{ width: 200, background: c.sidebar, borderRight: `1px solid ${c.border}`, display: 'flex', flexDirection: 'column', flexShrink: 0 }}>
        {/* Logo */}
        <div style={{ padding: '20px 20px 16px', borderBottom: `1px solid ${c.border}` }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
            <div style={{ width: 16, height: 16, border: `1.5px solid ${c.accent}`, position: 'relative', flexShrink: 0 }}>
              <div style={{ position: 'absolute', inset: 3, background: c.accent }} />
            </div>
            <span style={{ fontFamily: MONO, fontSize: 12, fontWeight: 600, letterSpacing: 1, textTransform: 'uppercase', color: c.textPrimary }}>animaOS</span>
          </div>
          <div style={{ display: 'flex', alignItems: 'center', gap: 5, marginTop: 6 }}>
            <span style={{ width: 6, height: 6, background: daemonOk ? c.success : c.danger, flexShrink: 0 }} />
            <span style={{ fontSize: 10, fontFamily: MONO, color: c.textMuted }}>{daemonOk ? 'daemon online' : 'daemon offline'}</span>
          </div>
        </div>

        {/* Nav */}
        <nav style={{ flex: 1, padding: '10px 0' }}>
          {NAV.map(item => {
            const active = view.name === item.id;
            return (
              <button key={item.id} onClick={() => setView({ name: item.id } as View)}
                style={{
                  width: '100%', display: 'flex', justifyContent: 'space-between', alignItems: 'center',
                  padding: '9px 20px', fontSize: 13, background: active ? c.accentLight : 'transparent',
                  border: 'none', borderLeft: `2px solid ${active ? c.accent : 'transparent'}`,
                  color: active ? c.accent : c.textSecondary, cursor: 'pointer',
                  fontFamily: SANS, textAlign: 'left',
                }}>
                {item.label}
                {item.count !== undefined && item.count > 0 && (
                  <span style={{ fontSize: 10, fontFamily: MONO, color: c.textMuted, background: c.subtle, padding: '1px 6px', border: `1px solid ${c.border}` }}>
                    {item.count}
                  </span>
                )}
              </button>
            );
          })}
        </nav>

        {/* Bottom controls */}
        <div style={{ padding: '14px 20px', borderTop: `1px solid ${c.border}`, display: 'flex', flexDirection: 'column', gap: 8 }}>
          <div style={{ display: 'flex', gap: 6 }}>
            <button onClick={() => setShowNewAgent(true)} style={{ flex: 1, padding: '7px 0', fontSize: 11, fontWeight: 600, background: c.accentSoft, color: c.accent, border: `1px solid ${c.accent}40`, cursor: 'pointer', fontFamily: MONO }}>+ Agent</button>
            <button onClick={() => setShowNewSwarm(true)} style={{ flex: 1, padding: '7px 0', fontSize: 11, fontWeight: 600, background: 'rgba(34,197,94,0.1)', color: c.success, border: `1px solid ${c.success}40`, cursor: 'pointer', fontFamily: MONO }}>+ Swarm</button>
          </div>
          <div style={{ display: 'flex', justifyContent: 'space-between' }}>
            <button onClick={() => setDark(d => !d)} style={{ fontSize: 10, fontFamily: MONO, background: 'transparent', border: 'none', color: c.textMuted, cursor: 'pointer', padding: 0 }}>
              {dark ? '☀ light' : '● dark'}
            </button>
            <button onClick={onSignOut} style={{ fontSize: 10, fontFamily: MONO, background: 'transparent', border: 'none', color: c.textMuted, cursor: 'pointer', padding: 0 }}>
              sign out
            </button>
          </div>
        </div>
      </aside>

      {/* ── Main ── */}
      <div style={{ flex: 1, display: 'flex', flexDirection: 'column', minWidth: 0 }}>
        {/* Top bar */}
        <header style={{ height: 44, borderBottom: `1px solid ${c.border}`, display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '0 24px', flexShrink: 0, background: c.elevated }}>
          <div style={{ fontSize: 11, fontFamily: MONO, color: c.textMuted }}>
            {view.name === 'agent' ? '← fleet / ' : ''}{view.name}
            {view.name === 'agent' && ` / ${agentList.find(a => a.state.id === (view as { id: string }).id)?.state.name ?? '…'}`}
          </div>
          <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
            {globalError && (
              <div style={{ fontSize: 11, color: c.danger, fontFamily: MONO, maxWidth: 360, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                ⚠ {globalError}
                <button onClick={() => setGlobalError(null)} style={{ marginLeft: 8, background: 'none', border: 'none', color: c.danger, cursor: 'pointer', fontSize: 11 }}>✕</button>
              </div>
            )}
            <button onClick={refresh} disabled={loading}
              style={{ fontSize: 10, fontFamily: MONO, padding: '4px 10px', background: 'transparent', border: `1px solid ${c.border}`, color: c.textMuted, cursor: 'pointer', opacity: loading ? 0.4 : 1 }}>
              {loading ? '…' : 'Refresh'}
            </button>
          </div>
        </header>

        {/* View area */}
        <div style={{ flex: 1, display: 'flex', minHeight: 0 }}>
          {view.name === 'overview' && (
            <ViewHome agents={agentList} swarms={swarmList} health={healthState} memories={recentMems} liveEvents={liveEvents} dark={dark} c={c} tick={tick} onNavigate={navigate} />
          )}
          {view.name === 'fleet' && (
            <ViewFleet agents={agentList} dark={dark} c={c} tick={tick}
              onCreateAgent={() => setShowNewAgent(true)}
              onRunAgent={a => setRunAgentTarget(a)}
              onDeleteAgent={handleDeleteAgent}
              deletingId={deletingId}
            />
          )}
          {view.name === 'swarms' && (
            <ViewSwarms swarms={swarmList} dark={dark} c={c} liveEvents={liveEvents} streamingId={streamingId}
              onCreateSwarm={() => setShowNewSwarm(true)}
              onRunSwarm={s => setRunSwarmTarget(s)}
              onToggleStream={toggleStream}
            />
          )}
          {view.name === 'messages' && (
            <ViewMessages agents={agentList} dark={dark} c={c} onNavigate={navigate} />
          )}
          {view.name === 'memory' && (
            <ViewMemory recentMemories={recentMems} dark={dark} c={c} />
          )}
          {view.name === 'agent' && (() => {
            const agent = agentList.find(a => a.state.id === (view as { id: string }).id);
            return agent
              ? <ViewAgentDetail agent={agent} allMemories={recentMems} dark={dark} c={c} tick={tick}
                  onBack={() => setView({ name: 'fleet' })}
                  onRun={() => setRunAgentTarget(agent)}
                />
              : <div style={{ padding: 40, color: c.textMuted, fontFamily: MONO }}>Agent not found</div>;
          })()}
        </div>
      </div>

      {/* ── Modals ── */}
      {showNewAgent && (
        <NewAgentModal configuredProviders={configuredProviders} c={c} onClose={() => setShowNewAgent(false)}
          onCreated={agent => { setAgentList(prev => [...prev, agent]); setShowNewAgent(false); }} />
      )}
      {showNewSwarm && (
        <NewSwarmModal configuredProviders={configuredProviders} c={c} onClose={() => setShowNewSwarm(false)}
          onCreated={swarm => { setSwarmList(prev => [...prev, swarm]); setShowNewSwarm(false); }} />
      )}
      {runAgentTarget && (
        <RunModal kind="agent" label={runAgentTarget.state.name} c={c}
          onClose={() => setRunAgentTarget(null)}
          onRun={task => runAgent(runAgentTarget.state.id, task)} />
      )}
      {runSwarmTarget && (
        <RunModal kind="swarm" label={runSwarmTarget.id} c={c}
          onClose={() => setRunSwarmTarget(null)}
          onRun={task => runSwarm(runSwarmTarget.id, task)} />
      )}

      {/* Pulse animation */}
      <style>{`@keyframes pulse { 0%,100% { opacity:1; } 50% { opacity:0.3; } }`}</style>
    </div>
  );
}
