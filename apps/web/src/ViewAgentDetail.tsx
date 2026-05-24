import React, { useMemo, useState } from 'react';
import { AgentSnapshot, Memory, agents as agentsApi, memories as memoriesApi } from './lib/api';
import { Colors, MONO, fmtTokens, relativeTime, genSeries, tokenLoad, statusColor } from './design';
import { AgentAvatar, MiniChart, StatusBadge } from './ui';

interface Props {
  agent: AgentSnapshot;
  allMemories: Memory[];
  dark: boolean;
  c: Colors;
  tick: number;
  onBack: () => void;
  onRun: () => void;
}

type DetailTab = 'overview' | 'messages' | 'memory';

export function ViewAgentDetail({ agent, allMemories, dark, c, tick, onBack, onRun }: Props) {
  const [tab, setTab] = useState<DetailTab>('overview');
  const { state, messageCount, eventCount, lastTask, messages } = agent;
  const st = state.status;
  const stColor = statusColor(st, dark);
  const load = tokenLoad(state.tokenUsage.totalTokens, state.config?.model ?? '');
  const series  = useMemo(() => genSeries(40, load, 0.06, 3 + tick), [tick, load]);
  const series2 = useMemo(() => genSeries(40, 0.4, 0.1, 17 + tick), [tick]);
  const agentMems = allMemories.filter(m => m.agentId === state.id);

  const TABS: { id: DetailTab; label: string }[] = [
    { id: 'overview',  label: 'Overview' },
    { id: 'messages',  label: `Messages (${messages?.length ?? messageCount})` },
    { id: 'memory',    label: `Memory (${agentMems.length})` },
  ];

  const ROLE_COLOR: Record<string, string> = {
    user: c.accent, assistant: c.success, system: c.warn, tool: '#a78bfa',
  };

  return (
    <div style={{ flex: 1, display: 'flex', flexDirection: 'column', minHeight: 0 }}>
      {/* Hero */}
      <div style={{ padding: '20px 28px', borderBottom: `1px solid ${c.border}`, flexShrink: 0 }}>
        <button onClick={onBack} style={{ background: 'transparent', border: 'none', color: c.textMuted, fontSize: 11, cursor: 'pointer', fontFamily: MONO, padding: '0 0 12px 0', display: 'flex', alignItems: 'center', gap: 5 }}>
          ← Fleet
        </button>

        <div style={{ display: 'grid', gridTemplateColumns: 'auto 1fr auto', gap: 20, alignItems: 'flex-start' }}>
          {/* Avatar */}
          <AgentAvatar name={state.name} size={80} status={st} dark={dark} c={c} />

          {/* Info */}
          <div>
            <div style={{ display: 'flex', alignItems: 'center', gap: 12, flexWrap: 'wrap' }}>
              <h1 style={{ fontSize: 28, fontWeight: 800, letterSpacing: -0.8, margin: 0 }}>{state.name}</h1>
              <StatusBadge status={st} dark={dark} c={c} size="md" />
            </div>
            <div style={{ fontSize: 11, color: c.textMuted, fontFamily: MONO, marginTop: 4 }}>
              {state.config?.provider} · {state.config?.model}
            </div>
            {state.config?.bio && (
              <p style={{ margin: '10px 0 0', fontSize: 13, color: c.textSecondary, lineHeight: 1.65, fontStyle: 'italic', borderLeft: `2px solid ${c.accent}`, paddingLeft: 12, maxWidth: 560 }}>
                "{state.config.bio}"
              </p>
            )}
            <div style={{ display: 'flex', gap: 5, marginTop: 10, flexWrap: 'wrap' }}>
              {(state.config?.adjectives ?? []).map(a => (
                <span key={a} style={{ fontSize: 10, padding: '3px 9px', border: `1px solid ${c.border}`, color: c.textMuted, fontFamily: MONO }}>{a}</span>
              ))}
              {(state.config?.topics ?? []).map(t => (
                <span key={t} style={{ fontSize: 10, padding: '3px 9px', background: c.accentSoft, color: c.accent, fontFamily: MONO }}>{t}</span>
              ))}
            </div>
          </div>

          {/* Quick stats */}
          <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', border: `1px solid ${c.border}`, alignSelf: 'start' }}>
            {[
              { k: 'Tokens',   v: fmtTokens(state.tokenUsage.totalTokens) },
              { k: 'Messages', v: messageCount.toLocaleString() },
              { k: 'Events',   v: eventCount.toLocaleString() },
              { k: 'Load',     v: `${(load * 100).toFixed(0)}%` },
            ].map(({ k, v }, i) => (
              <div key={k} style={{ padding: '10px 16px', borderRight: i % 2 === 0 ? `1px solid ${c.border}` : 'none', borderBottom: i < 2 ? `1px solid ${c.border}` : 'none' }}>
                <div style={{ fontSize: 9, color: c.textMuted, fontFamily: MONO, letterSpacing: 1.2, textTransform: 'uppercase', marginBottom: 3 }}>{k}</div>
                <div style={{ fontSize: 18, fontWeight: 700 }}>{v}</div>
              </div>
            ))}
          </div>
        </div>

        {/* Tab bar */}
        <div style={{ display: 'flex', marginTop: 20, borderBottom: `1px solid ${c.border}`, gap: 0 }}>
          {TABS.map(t => (
            <button key={t.id} onClick={() => setTab(t.id)} style={{
              padding: '10px 20px', fontSize: 13, cursor: 'pointer', fontFamily: 'inherit',
              background: 'transparent', border: 'none',
              borderBottom: `2px solid ${tab === t.id ? c.accent : 'transparent'}`,
              color: tab === t.id ? c.textPrimary : c.textMuted,
              fontWeight: tab === t.id ? 600 : 400, marginBottom: -1,
            }}>{t.label}</button>
          ))}
          <div style={{ flex: 1 }} />
          <button onClick={onRun} style={{ alignSelf: 'center', marginRight: 4, padding: '7px 16px', fontSize: 12, fontWeight: 600, cursor: 'pointer', background: c.accentSoft, color: c.accent, border: `1px solid ${c.accent}40` }}>
            Run task →
          </button>
        </div>
      </div>

      {/* Tab content */}
      <div style={{ flex: 1, overflow: 'auto' }}>

        {/* ── Overview ── */}
        {tab === 'overview' && (
          <div style={{ padding: '24px 28px', display: 'flex', flexDirection: 'column', gap: 16 }}>
            <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 14 }}>
              <MiniChart title="Token load · live" series={series} c={c} accent={c.accent} unit="%" mult={100} />
              <MiniChart title="Latency p50 · live" series={series2} c={c} accent={c.warn} unit="ms" mult={400} />
            </div>

            <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 14 }}>
              {/* Config */}
              <div style={{ padding: '16px', border: `1px solid ${c.border}` }}>
                <div style={{ fontSize: 9, color: c.textMuted, fontFamily: MONO, letterSpacing: 1.2, textTransform: 'uppercase', marginBottom: 12 }}>Configuration</div>
                <div style={{ display: 'flex', flexDirection: 'column', gap: 8, fontFamily: MONO, fontSize: 12 }}>
                  {[
                    ['provider', state.config?.provider ?? '—'],
                    ['model',    state.config?.model ?? '—'],
                    ['style',    state.config?.style ?? '—'],
                    ['id',       state.id.slice(0, 18) + '…'],
                  ].map(([k, v]) => (
                    <div key={k} style={{ display: 'flex', justifyContent: 'space-between', gap: 8, paddingBottom: 7, borderBottom: `1px solid ${c.border}` }}>
                      <span style={{ color: c.textMuted }}>{k}</span>
                      <span style={{ color: c.textPrimary }}>{v}</span>
                    </div>
                  ))}
                </div>
              </div>

              {/* Lore / last task */}
              <div style={{ padding: '16px', border: `1px solid ${c.border}` }}>
                {state.config?.lore ? (
                  <>
                    <div style={{ fontSize: 9, color: c.textMuted, fontFamily: MONO, letterSpacing: 1.2, textTransform: 'uppercase', marginBottom: 8 }}>Background</div>
                    <div style={{ fontSize: 13, color: c.textSecondary, lineHeight: 1.65 }}>{state.config.lore}</div>
                  </>
                ) : lastTask ? (
                  <>
                    <div style={{ fontSize: 9, color: c.textMuted, fontFamily: MONO, letterSpacing: 1.2, textTransform: 'uppercase', marginBottom: 8 }}>Last task</div>
                    <div style={{ fontSize: 13, color: c.textSecondary, lineHeight: 1.65, marginBottom: 10 }}>
                      {typeof lastTask.data === 'string' ? lastTask.data : lastTask.error ?? JSON.stringify(lastTask.data)}
                    </div>
                    <StatusBadge status={lastTask.status} dark={dark} c={c} />
                  </>
                ) : (
                  <div style={{ fontSize: 12, color: c.textMuted, fontFamily: MONO }}>No background info.</div>
                )}
              </div>
            </div>
          </div>
        )}

        {/* ── Messages ── */}
        {tab === 'messages' && (
          <div style={{ padding: '20px 28px' }}>
            {!messages || messages.length === 0 ? (
              <div style={{ color: c.textMuted, fontFamily: MONO, fontSize: 12, padding: '40px 0', textAlign: 'center' }}>No messages recorded</div>
            ) : (
              <div style={{ border: `1px solid ${c.border}` }}>
                {[...messages].reverse().map((msg, i) => (
                  <div key={msg.id} style={{ padding: '14px 16px', borderBottom: i < messages.length - 1 ? `1px solid ${c.border}` : 'none',
                    borderLeft: `3px solid ${ROLE_COLOR[msg.role] ?? c.textMuted}` }}>
                    <div style={{ display: 'flex', justifyContent: 'space-between', marginBottom: 6 }}>
                      <span style={{ fontSize: 9, padding: '2px 8px', fontFamily: MONO, letterSpacing: 0.8, textTransform: 'uppercase',
                        color: ROLE_COLOR[msg.role] ?? c.textMuted, background: (ROLE_COLOR[msg.role] ?? c.textMuted) + '15',
                        border: `1px solid ${(ROLE_COLOR[msg.role] ?? c.textMuted)}30` }}>{msg.role}</span>
                      <span style={{ fontSize: 10, color: c.textMuted, fontFamily: MONO }}>{relativeTime(msg.createdAtMs)}</span>
                    </div>
                    <div style={{ fontSize: 13, color: c.textSecondary, lineHeight: 1.65, whiteSpace: 'pre-wrap', wordBreak: 'break-word' }}>
                      {String(msg.content?.text ?? '').slice(0, 600)}
                    </div>
                  </div>
                ))}
              </div>
            )}
          </div>
        )}

        {/* ── Memory ── */}
        {tab === 'memory' && (
          <div style={{ padding: '20px 28px' }}>
            {agentMems.length === 0 ? (
              <div style={{ color: c.textMuted, fontFamily: MONO, fontSize: 12, padding: '40px 0', textAlign: 'center' }}>No memories recorded</div>
            ) : (
              <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
                {[...agentMems].sort((a, b) => b.importance - a.importance).map(mem => {
                  const typeColor = mem.type === 'fact' ? '#0ea5e9' : mem.type === 'reflection' ? '#f59e0b' : '#8b5cf6';
                  return (
                    <div key={mem.id} style={{ padding: '14px 16px', border: `1px solid ${c.border}`, borderLeft: `3px solid ${typeColor}` }}>
                      <div style={{ display: 'flex', justifyContent: 'space-between', marginBottom: 8 }}>
                        <div style={{ display: 'flex', gap: 6 }}>
                          <span style={{ fontSize: 9, padding: '2px 7px', fontFamily: MONO, color: typeColor, background: typeColor + '15', border: `1px solid ${typeColor}30` }}>{mem.type}</span>
                          <span style={{ fontSize: 9, padding: '2px 7px', fontFamily: MONO, color: c.textMuted, border: `1px solid ${c.border}` }}>{mem.scope}</span>
                        </div>
                        <span style={{ fontSize: 11, fontFamily: MONO, color: c.textMuted }}>{(mem.importance * 100).toFixed(0)}% · {relativeTime(mem.createdAt)}</span>
                      </div>
                      <div style={{ fontSize: 13, color: c.textSecondary, lineHeight: 1.65 }}>{mem.content}</div>
                      {(mem.tags ?? []).length > 0 && (
                        <div style={{ display: 'flex', gap: 4, marginTop: 8, flexWrap: 'wrap' }}>
                          {mem.tags!.map(t => (
                            <span key={t} style={{ fontSize: 9, padding: '1px 6px', background: c.subtle, color: c.textMuted, border: `1px solid ${c.border}`, fontFamily: MONO }}>#{t}</span>
                          ))}
                        </div>
                      )}
                    </div>
                  );
                })}
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
