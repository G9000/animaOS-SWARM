import React, { useState } from 'react';
import { AgentSnapshot } from './lib/api';
import { Colors, MONO, fmtTokens, relativeTime, avatarUrl, tokenLoad, statusColor, genSeries } from './design';
import { AgentAvatar, StatusBadge, MiniChart } from './ui';

interface Props {
  agents: AgentSnapshot[];
  dark: boolean;
  c: Colors;
  tick: number;
  onCreateAgent: () => void;
  onRunAgent: (agent: AgentSnapshot) => void;
  onDeleteAgent: (id: string) => void;
  deletingId: string | null;
}

export function ViewFleet({ agents, dark, c, tick, onCreateAgent, onRunAgent, onDeleteAgent, deletingId }: Props) {
  const [selected, setSelected] = useState<string>(agents[0]?.state.id ?? '');
  const sorted = [...agents].sort((a, b) => b.messageCount - a.messageCount);
  const sel = agents.find(a => a.state.id === selected) ?? agents[0];

  const running    = agents.filter(a => a.state.status === 'running').length;
  const idle       = agents.filter(a => a.state.status === 'idle').length;
  const attention  = agents.filter(a => ['failed', 'terminated'].includes(a.state.status)).length;

  return (
    <div style={{ flex: 1, display: 'grid', gridTemplateColumns: '1fr 340px', minHeight: 0 }}>
      {/* Table pane */}
      <div style={{ display: 'flex', flexDirection: 'column', minHeight: 0, overflow: 'hidden' }}>
        {/* Header */}
        <div style={{ padding: '20px 28px 16px', borderBottom: `1px solid ${c.border}`,
          display: 'flex', justifyContent: 'space-between', alignItems: 'center', flexShrink: 0 }}>
          <div>
            <div style={{ fontWeight: 700, fontSize: 20, letterSpacing: -0.4 }}>Fleet</div>
            <div style={{ fontSize: 11, color: c.textMuted, marginTop: 2, fontFamily: MONO }}>
              {running} running · {idle} idle · {attention} attention
            </div>
          </div>
          <button onClick={onCreateAgent} style={{
            padding: '8px 16px', fontSize: 12, fontWeight: 600, color: dark ? '#0f1115' : '#fff',
            background: c.accent, border: 'none', cursor: 'pointer',
          }}>+ New agent</button>
        </div>

        {/* Table */}
        <div style={{ flex: 1, overflow: 'auto' }}>
          <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: 13 }}>
            <thead style={{ position: 'sticky', top: 0, zIndex: 1 }}>
              <tr style={{ background: c.subtle }}>
                {['', 'Agent', 'Model', 'Tokens', 'Messages', 'Status', ''].map((h, i) => (
                  <th key={i} style={{ padding: '10px 16px', textAlign: 'left', fontWeight: 500,
                    fontSize: 9, color: c.textMuted, borderBottom: `1px solid ${c.border}`,
                    fontFamily: MONO, letterSpacing: 1.2, textTransform: 'uppercase', whiteSpace: 'nowrap' }}>{h}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {sorted.map(agent => (
                <FleetRow
                  key={agent.state.id}
                  agent={agent}
                  selected={selected === agent.state.id}
                  c={c} dark={dark}
                  onSelect={() => setSelected(agent.state.id)}
                  onRun={() => onRunAgent(agent)}
                  onDelete={() => onDeleteAgent(agent.state.id)}
                  deleting={deletingId === agent.state.id}
                />
              ))}
            </tbody>
          </table>
        </div>
      </div>

      {/* Detail sidebar */}
      {sel ? (
        <AgentDetailSidebar agent={sel} c={c} dark={dark} tick={tick} onRun={() => onRunAgent(sel)} />
      ) : (
        <div style={{ borderLeft: `1px solid ${c.border}`, background: c.sidebar }} />
      )}
    </div>
  );
}

// ── Fleet row ─────────────────────────────────────────────────────────────────
function FleetRow({ agent, selected, c, dark, onSelect, onRun, onDelete, deleting }: {
  agent: AgentSnapshot; selected: boolean; c: Colors; dark: boolean;
  onSelect: () => void; onRun: () => void; onDelete: () => void; deleting: boolean;
}) {
  const [hover, setHover] = useState(false);
  const { state, messageCount } = agent;
  const st = state.status;

  return (
    <tr onClick={onSelect}
      onMouseEnter={() => setHover(true)} onMouseLeave={() => setHover(false)}
      style={{
        borderBottom: `1px solid ${c.border}`, cursor: 'pointer',
        borderLeft: `2px solid ${selected ? c.accent : 'transparent'}`,
        background: selected
          ? (dark ? 'rgba(56,189,248,0.05)' : 'rgba(234,88,12,0.04)')
          : hover ? c.subtle : 'transparent',
      }}>
      {/* Avatar */}
      <td style={{ padding: '10px 0 10px 14px', width: 48 }}>
        <AgentAvatar name={state.name} size={34} status={st} dark={dark} c={c} />
      </td>
      {/* Name + bio */}
      <td style={{ padding: '10px 16px' }}>
        <div style={{ fontWeight: 600, fontSize: 13, color: selected ? c.accent : c.textPrimary }}>{state.name}</div>
        <div style={{ fontSize: 10, color: c.textMuted, marginTop: 1, maxWidth: 220, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
          {state.config?.bio?.split('.')[0] ?? '—'}
        </div>
      </td>
      {/* Model */}
      <td style={{ padding: '10px 16px', fontFamily: MONO, fontSize: 11 }}>
        <div style={{ color: c.textMuted }}>{state.config?.provider ?? '—'}</div>
        <div style={{ fontSize: 10, color: c.textMuted, opacity: 0.7 }}>
          {(state.config?.model ?? '—').replace('claude-', '').replace('gpt-', '')}
        </div>
      </td>
      {/* Tokens */}
      <td style={{ padding: '10px 16px', fontFamily: MONO, fontSize: 12 }}>
        {fmtTokens(state.tokenUsage.totalTokens)}
      </td>
      {/* Messages */}
      <td style={{ padding: '10px 16px', fontFamily: MONO, fontSize: 12, color: c.textMuted }}>
        {messageCount.toLocaleString()}
      </td>
      {/* Status */}
      <td style={{ padding: '10px 16px' }}>
        <StatusBadge status={st} dark={dark} c={c} />
      </td>
      {/* Actions */}
      <td style={{ padding: '10px 14px', whiteSpace: 'nowrap' }}>
        <div style={{ display: 'flex', gap: 6 }}>
          <button onClick={e => { e.stopPropagation(); onRun(); }}
            style={{ fontSize: 10, fontFamily: MONO, padding: '3px 8px', background: c.accentSoft, color: c.accent, border: `1px solid ${c.accent}40`, cursor: 'pointer' }}>
            Run
          </button>
          <button onClick={e => { e.stopPropagation(); onDelete(); }} disabled={deleting}
            style={{ fontSize: 10, fontFamily: MONO, padding: '3px 8px', background: 'transparent', color: c.danger, border: `1px solid ${c.danger}40`, cursor: 'pointer', opacity: deleting ? 0.5 : 1 }}>
            {deleting ? '…' : 'Del'}
          </button>
        </div>
      </td>
    </tr>
  );
}

// ── Agent detail sidebar ──────────────────────────────────────────────────────
function AgentDetailSidebar({ agent, c, dark, tick, onRun }: {
  agent: AgentSnapshot; c: Colors; dark: boolean; tick: number; onRun: () => void;
}) {
  const { state, messageCount, eventCount, lastTask } = agent;
  const st = state.status;
  const stColor = statusColor(st, dark);
  const load = tokenLoad(state.tokenUsage.totalTokens, state.config?.model ?? '');
  const series = React.useMemo(() => genSeries(40, load, 0.06, 3 + tick), [tick, load]);
  const series2 = React.useMemo(() => genSeries(40, 0.4, 0.1, 17 + tick), [tick]);

  return (
    <div style={{ borderLeft: `1px solid ${c.border}`, background: c.sidebar, display: 'flex', flexDirection: 'column', overflow: 'auto' }}>
      {/* Hero */}
      <div style={{ padding: '24px 24px 20px', borderBottom: `1px solid ${c.border}` }}>
        <div style={{ display: 'flex', gap: 16, alignItems: 'flex-start' }}>
          <AgentAvatar name={state.name} size={64} status={st} dark={dark} c={c} />
          <div style={{ flex: 1, minWidth: 0 }}>
            <div style={{ display: 'flex', alignItems: 'baseline', justifyContent: 'space-between', gap: 8 }}>
              <div style={{ fontSize: 18, fontWeight: 700 }}>{state.name}</div>
              <StatusBadge status={st} dark={dark} c={c} />
            </div>
            <div style={{ fontSize: 11, color: c.textMuted, fontFamily: MONO, marginTop: 3 }}>
              {state.config?.provider} · {state.config?.model}
            </div>
            <div style={{ display: 'flex', gap: 5, marginTop: 8, flexWrap: 'wrap' }}>
              {(state.config?.adjectives ?? []).map(a => (
                <span key={a} style={{ fontSize: 9, padding: '2px 7px', border: `1px solid ${c.border}`, color: c.textMuted, fontFamily: MONO }}>{a}</span>
              ))}
            </div>
          </div>
        </div>
        {state.config?.bio && (
          <p style={{ margin: '14px 0 0', fontSize: 12, color: c.textSecondary, lineHeight: 1.65, fontStyle: 'italic', borderLeft: `2px solid ${c.accent}`, paddingLeft: 12 }}>
            "{state.config.bio}"
          </p>
        )}
        <button onClick={onRun} style={{ marginTop: 14, width: '100%', padding: '9px 0', fontSize: 12, fontWeight: 600, cursor: 'pointer', background: c.accentSoft, color: c.accent, border: `1px solid ${c.accent}40` }}>
          Run task →
        </button>
      </div>

      {/* Stats grid */}
      <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', borderBottom: `1px solid ${c.border}` }}>
        {[
          { k: 'Prompt',     v: fmtTokens(state.tokenUsage.promptTokens) },
          { k: 'Completion', v: fmtTokens(state.tokenUsage.completionTokens) },
          { k: 'Messages',   v: messageCount.toLocaleString() },
          { k: 'Events',     v: eventCount.toLocaleString() },
        ].map(({ k, v }, i) => (
          <div key={k} style={{ padding: '14px 20px', borderRight: i % 2 === 0 ? `1px solid ${c.border}` : 'none', borderBottom: i < 2 ? `1px solid ${c.border}` : 'none' }}>
            <div style={{ fontSize: 9, color: c.textMuted, fontFamily: MONO, letterSpacing: 1.2, textTransform: 'uppercase', marginBottom: 4 }}>{k}</div>
            <div style={{ fontSize: 20, fontWeight: 700 }}>{v}</div>
          </div>
        ))}
      </div>

      {/* Mini charts */}
      <div style={{ padding: '14px', display: 'flex', flexDirection: 'column', gap: 10, borderBottom: `1px solid ${c.border}` }}>
        <MiniChart title="Token load" series={series} c={c} accent={c.accent} unit="%" mult={100} />
        <MiniChart title="Latency p50" series={series2} c={c} accent={c.warn} unit="ms" mult={400} />
      </div>

      {/* Topics */}
      {(state.config?.topics ?? []).length > 0 && (
        <div style={{ padding: '16px 20px', borderBottom: `1px solid ${c.border}` }}>
          <div style={{ fontSize: 9, color: c.textMuted, fontFamily: MONO, letterSpacing: 1.2, textTransform: 'uppercase', marginBottom: 10 }}>Topics</div>
          <div style={{ display: 'flex', gap: 5, flexWrap: 'wrap' }}>
            {state.config!.topics!.map(t => (
              <span key={t} style={{ fontSize: 10, padding: '3px 8px', background: c.accentSoft, color: c.accent, fontFamily: MONO, border: `1px solid ${c.accent}30` }}>{t}</span>
            ))}
          </div>
        </div>
      )}

      {/* Last task */}
      {lastTask && (
        <div style={{ padding: '16px 20px' }}>
          <div style={{ fontSize: 9, color: c.textMuted, fontFamily: MONO, letterSpacing: 1.2, textTransform: 'uppercase', marginBottom: 8 }}>Last task</div>
          <div style={{ fontSize: 12, color: c.textSecondary, lineHeight: 1.6 }}>
            {typeof lastTask.data === 'object' && lastTask.data !== null
              ? JSON.stringify(lastTask.data).slice(0, 140)
              : lastTask.error ?? 'No output'}
          </div>
          <div style={{ marginTop: 8 }}>
            <StatusBadge status={lastTask.status} dark={dark} c={c} />
          </div>
        </div>
      )}
    </div>
  );
}
