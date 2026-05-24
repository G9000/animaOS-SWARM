import React, { useMemo, useState } from 'react';
import { AgentSnapshot, HealthResponse, Memory, SwarmState, SwarmStreamEvent } from './lib/api';
import { Colors, MONO, fmtTokens, relativeTime, genSeries, EVENT_TYPE_COLOR, tokenLoad } from './design';
import { ActivityChart, AgentAvatar, MiniTopology, PanelHeader, StatStrip } from './ui';

interface Props {
  agents: AgentSnapshot[];
  swarms: SwarmState[];
  health: HealthResponse | null;
  memories: Memory[];
  liveEvents: SwarmStreamEvent[];
  dark: boolean;
  c: Colors;
  tick: number;
  onNavigate: (view: string, params?: Record<string, string>) => void;
}

export function ViewHome({ agents, swarms, health, dark, c, tick, liveEvents, onNavigate }: Props) {
  const series1 = useMemo(() => genSeries(48, 0.55, 0.1, 42 + tick), [tick]);
  const series2 = useMemo(() => genSeries(48, 0.38, 0.12, 17 + tick), [tick]);

  const running    = agents.filter(a => a.state.status === 'running').length;
  const failed     = agents.filter(a => a.state.status === 'failed').length;
  const totalTok   = agents.reduce((s, a) => s + a.state.tokenUsage.totalTokens, 0);
  const totalMsgs  = agents.reduce((s, a) => s + a.messageCount, 0);
  const daemonOk   = health?.status === 'ok';

  return (
    <div style={{ flex: 1, display: 'flex', flexDirection: 'column', minHeight: 0, overflow: 'auto' }}>
      {/* Page header */}
      <div style={{ padding: '20px 28px 16px', borderBottom: `1px solid ${c.border}`, flexShrink: 0 }}>
        <div style={{ fontWeight: 700, fontSize: 20, letterSpacing: -0.4 }}>Overview</div>
        <div style={{ fontSize: 11, color: c.textMuted, marginTop: 2, fontFamily: MONO }}>
          {new Date().toLocaleString('en-GB', { dateStyle: 'medium', timeStyle: 'short' })} ·{' '}
          daemon <span style={{ color: daemonOk ? c.success : c.danger }}>{daemonOk ? 'online' : 'offline'}</span>
          {health?.version && ` · v${health.version}`}
        </div>
      </div>

      <div style={{ flex: 1, padding: '20px 28px', display: 'flex', flexDirection: 'column', gap: 16 }}>
        {/* Stats */}
        <StatStrip c={c} stats={[
          { label: 'Agents',       value: agents.length,         sub: `${running} running`,           highlight: c.accent },
          { label: 'Total tokens', value: fmtTokens(totalTok),  sub: 'all time' },
          { label: 'Messages',     value: fmtTokens(totalMsgs), sub: 'all agents' },
          { label: 'Swarms',       value: swarms.length,         sub: `${swarms.filter(s => s.status === 'running').length} active` },
          { label: 'Attention',    value: failed,                sub: failed > 0 ? 'needs review' : 'all clear', highlight: failed > 0 ? c.danger : c.success },
        ]} />

        {/* Charts row */}
        <div style={{ display: 'grid', gridTemplateColumns: '1fr 360px', gap: 16 }}>
          {/* Activity */}
          <div style={{ border: `1px solid ${c.border}`, background: c.elevated }}>
            <div style={{ padding: '14px 18px', borderBottom: `1px solid ${c.border}`,
              display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
              <div>
                <div style={{ fontSize: 12, fontWeight: 600 }}>Activity</div>
                <div style={{ fontSize: 10, color: c.textMuted, fontFamily: MONO, marginTop: 1 }}>tasks · 24h · 30-min buckets</div>
              </div>
              <div style={{ display: 'flex', gap: 14, fontSize: 10, fontFamily: MONO }}>
                <span style={{ display: 'flex', alignItems: 'center', gap: 5, color: c.textMuted }}>
                  <span style={{ width: 12, height: 2, background: c.accent, display: 'inline-block' }} /> tasks/min
                </span>
                <span style={{ display: 'flex', alignItems: 'center', gap: 5, color: c.textMuted }}>
                  <span style={{ width: 12, height: 2, background: c.textMuted, display: 'inline-block', opacity: 0.5 }} /> latency
                </span>
              </div>
            </div>
            <div style={{ padding: '16px 18px 28px', position: 'relative', height: 168 }}>
              <ActivityChart series={series1} series2={series2} c={c} />
              <div style={{ position: 'absolute', bottom: 8, left: 18, right: 18,
                display: 'flex', justifyContent: 'space-between',
                fontSize: 9, color: c.textMuted, fontFamily: MONO }}>
                {['−24h', '−18h', '−12h', '−6h', 'now'].map(l => <span key={l}>{l}</span>)}
              </div>
            </div>
          </div>

          {/* Topology */}
          <div style={{ border: `1px solid ${c.border}`, background: c.elevated }}>
            <PanelHeader title="Topology" sub="agent mesh · live" c={c} />
            <div style={{ padding: '12px' }}>
              <MiniTopology agents={agents.map(a => ({ name: a.state.name, status: a.state.status }))} c={c} dark={dark} tick={tick} />
            </div>
          </div>
        </div>

        {/* Bottom row */}
        <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr 300px', gap: 16 }}>
          {/* Active agents */}
          <div style={{ border: `1px solid ${c.border}`, background: c.elevated }}>
            <PanelHeader title="Active agents" c={c}
              actions={
                <button onClick={() => onNavigate('fleet')} style={{ fontSize: 10, fontFamily: MONO, color: c.accent, background: 'transparent', border: 'none', cursor: 'pointer' }}>
                  View all →
                </button>
              }
            />
            <div>
              {agents.filter(a => a.state.status === 'running').slice(0, 7).map(agent => (
                <AgentMiniRow key={agent.state.id} agent={agent} c={c} dark={dark}
                  onClick={() => onNavigate('agent', { id: agent.state.id })} />
              ))}
              {agents.filter(a => a.state.status === 'running').length === 0 && (
                <div style={{ padding: '20px 16px', color: c.textMuted, fontSize: 12, fontFamily: MONO }}>No running agents</div>
              )}
            </div>
          </div>

          {/* Swarms */}
          <div style={{ border: `1px solid ${c.border}`, background: c.elevated }}>
            <PanelHeader title="Swarms" sub={`${swarms.length} total`} c={c}
              actions={
                <button onClick={() => onNavigate('swarms')} style={{ fontSize: 10, fontFamily: MONO, color: c.accent, background: 'transparent', border: 'none', cursor: 'pointer' }}>
                  View all →
                </button>
              }
            />
            <div>
              {swarms.slice(0, 6).map(sw => (
                <SwarmMiniRow key={sw.id} swarm={sw} c={c} dark={dark} />
              ))}
              {swarms.length === 0 && (
                <div style={{ padding: '20px 16px', color: c.textMuted, fontSize: 12, fontFamily: MONO }}>No swarms yet</div>
              )}
            </div>
          </div>

          {/* Live events */}
          <div style={{ border: `1px solid ${c.border}`, background: c.elevated }}>
            <PanelHeader title="Live events" c={c}
              actions={<LiveDot c={c} />}
            />
            <div style={{ padding: '10px 14px', display: 'flex', flexDirection: 'column', gap: 8 }}>
              {liveEvents.slice(0, 10).map((ev, i) => {
                const payload = ev.data as Record<string, unknown>;
                const agentName = String(payload.agentName ?? '');
                const text = (() => {
                  if ('message' in payload) {
                    const msg = payload.message as Record<string, unknown>;
                    const content = msg?.content as Record<string, unknown>;
                    return String(content?.text ?? ev.event).slice(0, 60);
                  }
                  return ev.event;
                })();
                return (
                  <EventRow key={i} type={ev.event} agent={agentName} text={text} c={c} opacity={Math.max(0.3, 1 - i * 0.08)} />
                );
              })}
              {liveEvents.length === 0 && (
                <div style={{ padding: '12px 0', color: c.textMuted, fontSize: 11, fontFamily: MONO }}>Waiting for events…</div>
              )}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

// ── Sub-components ────────────────────────────────────────────────────────────

function AgentMiniRow({ agent, c, dark, onClick }: { agent: AgentSnapshot; c: Colors; dark: boolean; onClick: () => void }) {
  const [hover, setHover] = useState(false);
  const load = tokenLoad(agent.state.tokenUsage.totalTokens, agent.state.config?.model ?? '');
  return (
    <div onMouseEnter={() => setHover(true)} onMouseLeave={() => setHover(false)}
      onClick={onClick}
      style={{ padding: '10px 16px', borderBottom: `1px solid ${c.border}`,
        background: hover ? c.subtle : 'transparent', cursor: 'pointer',
        display: 'flex', alignItems: 'center', gap: 10 }}>
      <AgentAvatar name={agent.state.name} size={28} status={agent.state.status} dark={dark} c={c} />
      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={{ fontSize: 12, fontWeight: 600 }}>{agent.state.name}</div>
        <div style={{ height: 3, background: c.border, marginTop: 5 }}>
          <div style={{ width: `${load * 100}%`, height: '100%', background: load > 0.85 ? c.warn : c.accent, transition: 'width 0.6s' }} />
        </div>
      </div>
      <div style={{ fontSize: 10, fontFamily: MONO, color: c.textMuted, flexShrink: 0 }}>{(load * 100).toFixed(0)}%</div>
    </div>
  );
}

function SwarmMiniRow({ swarm, c, dark }: { swarm: SwarmState; c: Colors; dark: boolean }) {
  const sc = swarm.status === 'running' ? c.success : swarm.status === 'completed' ? c.textMuted : c.danger;
  return (
    <div style={{ padding: '10px 16px', borderBottom: `1px solid ${c.border}`, display: 'flex', alignItems: 'center', gap: 10 }}>
      <span style={{ width: 7, height: 7, background: sc, flexShrink: 0 }} />
      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={{ fontSize: 12, fontWeight: 600, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{swarm.id.slice(0, 20)}</div>
        <div style={{ fontSize: 10, color: c.textMuted, fontFamily: MONO, marginTop: 1 }}>{swarm.agentIds?.length ?? 0} agents · {fmtTokens(swarm.tokenUsage?.totalTokens ?? 0)} tok</div>
      </div>
      <span style={{ fontSize: 9, fontFamily: MONO, color: sc, textTransform: 'uppercase', letterSpacing: 0.8 }}>{swarm.status}</span>
    </div>
  );
}

function EventRow({ type, agent, text, c, opacity }: { type: string; agent: string; text: string; c: Colors; opacity: number }) {
  const color = EVENT_TYPE_COLOR[type] ?? c.textMuted;
  return (
    <div style={{ display: 'flex', gap: 8, alignItems: 'flex-start', opacity }}>
      <span style={{ width: 5, height: 5, background: color, flexShrink: 0, marginTop: 4 }} />
      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={{ display: 'flex', justifyContent: 'space-between', gap: 4 }}>
          <span style={{ fontSize: 11, fontWeight: 600 }}>{agent || type.split(':')[0]}</span>
          <span style={{ fontSize: 9, color: c.textMuted, fontFamily: MONO, flexShrink: 0 }}>{type}</span>
        </div>
        <div style={{ fontSize: 10, color: c.textMuted, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{text}</div>
      </div>
    </div>
  );
}

function LiveDot({ c }: { c: Colors }) {
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 5, fontSize: 10, fontFamily: MONO, color: c.textMuted }}>
      <span style={{ width: 6, height: 6, background: c.success, borderRadius: '50%', animation: 'pulse 1.8s infinite' }} />
      live
    </div>
  );
}
