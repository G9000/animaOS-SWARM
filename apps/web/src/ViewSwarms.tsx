import React, { useState } from 'react';
import { SwarmState, SwarmStreamEvent } from './lib/api';
import { Colors, MONO, fmtTokens, relativeTime, statusColor, EVENT_TYPE_COLOR } from './design';
import { StatusBadge } from './ui';

interface Props {
  swarms: SwarmState[];
  dark: boolean;
  c: Colors;
  liveEvents: SwarmStreamEvent[];
  streamingId: string | null;
  onCreateSwarm: () => void;
  onRunSwarm: (swarm: SwarmState) => void;
  onToggleStream: (id: string) => void;
}

export function ViewSwarms({ swarms, dark, c, liveEvents, streamingId, onCreateSwarm, onRunSwarm, onToggleStream }: Props) {
  const [selected, setSelected] = useState<string | null>(swarms[0]?.id ?? null);
  const sel = swarms.find(s => s.id === selected);

  return (
    <div style={{ flex: 1, display: 'flex', flexDirection: 'column', minHeight: 0 }}>
      {/* Header */}
      <div style={{ padding: '20px 28px 16px', borderBottom: `1px solid ${c.border}`,
        display: 'flex', justifyContent: 'space-between', alignItems: 'center', flexShrink: 0 }}>
        <div>
          <div style={{ fontWeight: 700, fontSize: 20, letterSpacing: -0.4 }}>Swarms</div>
          <div style={{ fontSize: 11, color: c.textMuted, marginTop: 2, fontFamily: MONO }}>
            {swarms.filter(s => s.status === 'running').length} active · {swarms.length} total
          </div>
        </div>
        <button onClick={onCreateSwarm} style={{
          padding: '8px 16px', fontSize: 12, fontWeight: 600, color: dark ? '#0f1115' : '#fff',
          background: c.success, border: 'none', cursor: 'pointer',
        }}>+ New swarm</button>
      </div>

      {swarms.length === 0 ? (
        <EmptyState c={c} label="No swarms yet" sub="Create a swarm to coordinate multiple agents on a task." />
      ) : (
        <div style={{ flex: 1, display: 'grid', gridTemplateColumns: '1fr 380px', minHeight: 0 }}>
          {/* List */}
          <div style={{ overflow: 'auto' }}>
            {swarms.map(sw => (
              <SwarmCard key={sw.id} swarm={sw} selected={selected === sw.id}
                c={c} dark={dark}
                onSelect={() => setSelected(sw.id)}
                onRun={() => onRunSwarm(sw)}
                streaming={streamingId === sw.id}
                onToggleStream={() => onToggleStream(sw.id)}
              />
            ))}
          </div>
          {/* Detail */}
          {sel && (
            <SwarmDetail swarm={sel} c={c} dark={dark}
              liveEvents={liveEvents} streaming={streamingId === sel.id}
              onToggleStream={() => onToggleStream(sel.id)}
            />
          )}
        </div>
      )}
    </div>
  );
}

// ── Swarm card ────────────────────────────────────────────────────────────────
function SwarmCard({ swarm, selected, c, dark, onSelect, onRun, streaming, onToggleStream }: {
  swarm: SwarmState; selected: boolean; c: Colors; dark: boolean;
  onSelect: () => void; onRun: () => void; streaming: boolean; onToggleStream: () => void;
}) {
  const [hover, setHover] = useState(false);
  const sc = statusColor(swarm.status, dark);
  const totalTok = swarm.tokenUsage?.totalTokens ?? 0;

  return (
    <div onClick={onSelect}
      onMouseEnter={() => setHover(true)} onMouseLeave={() => setHover(false)}
      style={{
        padding: '16px 24px', borderBottom: `1px solid ${c.border}`, cursor: 'pointer',
        borderLeft: `2px solid ${selected ? c.accent : 'transparent'}`,
        background: selected ? c.accentLight : hover ? c.subtle : 'transparent',
      }}>
      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start', gap: 12 }}>
        <div style={{ flex: 1, minWidth: 0 }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 4 }}>
            <span style={{ width: 7, height: 7, background: sc, flexShrink: 0,
              ...(swarm.status === 'running' ? { animation: 'pulse 1.8s infinite' } : {}) }} />
            <div style={{ fontFamily: MONO, fontSize: 12, fontWeight: 600, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
              {swarm.id}
            </div>
          </div>
          <div style={{ display: 'flex', gap: 16, fontSize: 11, fontFamily: MONO, color: c.textMuted }}>
            <span>{swarm.agentIds?.length ?? 0} agents</span>
            <span>{swarm.messages?.length ?? 0} msgs</span>
            <span>{fmtTokens(totalTok)} tok</span>
          </div>
        </div>
        <div style={{ display: 'flex', gap: 6, flexShrink: 0 }}>
          <StatusBadge status={swarm.status} dark={dark} c={c} />
        </div>
      </div>
      {selected && (
        <div style={{ display: 'flex', gap: 8, marginTop: 12 }} onClick={e => e.stopPropagation()}>
          <button onClick={onRun} style={{ fontSize: 11, fontFamily: MONO, padding: '4px 12px', cursor: 'pointer', background: c.accentSoft, color: c.accent, border: `1px solid ${c.accent}40` }}>Run task</button>
          <button onClick={onToggleStream} style={{ fontSize: 11, fontFamily: MONO, padding: '4px 12px', cursor: 'pointer',
            background: streaming ? 'rgba(34,197,94,0.1)' : 'transparent',
            color: streaming ? c.success : c.textMuted, border: `1px solid ${streaming ? c.success + '40' : c.border}` }}>
            {streaming ? '⏹ Stop stream' : '▶ Live stream'}
          </button>
        </div>
      )}
    </div>
  );
}

// ── Swarm detail ──────────────────────────────────────────────────────────────
function SwarmDetail({ swarm, c, dark, liveEvents, streaming, onToggleStream }: {
  swarm: SwarmState; c: Colors; dark: boolean;
  liveEvents: SwarmStreamEvent[]; streaming: boolean; onToggleStream: () => void;
}) {
  const [tab, setTab] = useState<'messages' | 'events' | 'results'>('messages');
  const TABS = [
    { id: 'messages' as const, label: `Messages (${swarm.messages?.length ?? 0})` },
    { id: 'events'   as const, label: 'Live events' },
    { id: 'results'  as const, label: `Results (${swarm.results?.length ?? 0})` },
  ];

  return (
    <div style={{ borderLeft: `1px solid ${c.border}`, background: c.sidebar, display: 'flex', flexDirection: 'column', overflow: 'hidden' }}>
      {/* Header */}
      <div style={{ padding: '18px 20px', borderBottom: `1px solid ${c.border}`, flexShrink: 0 }}>
        <div style={{ fontFamily: MONO, fontSize: 11, color: c.textMuted, marginBottom: 4 }}>{swarm.id}</div>
        <div style={{ display: 'flex', gap: 16, fontSize: 11, fontFamily: MONO, color: c.textMuted }}>
          <span>{swarm.agentIds?.length ?? 0} agents</span>
          <span>{fmtTokens(swarm.tokenUsage?.totalTokens ?? 0)} tokens</span>
          {swarm.startedAt && <span>started {relativeTime(swarm.startedAt)}</span>}
        </div>
      </div>

      {/* Tabs */}
      <div style={{ display: 'flex', borderBottom: `1px solid ${c.border}`, flexShrink: 0 }}>
        {TABS.map(t => (
          <button key={t.id} onClick={() => setTab(t.id)} style={{
            padding: '9px 14px', fontSize: 11, fontFamily: MONO, cursor: 'pointer',
            background: 'transparent', border: 'none',
            borderBottom: `2px solid ${tab === t.id ? c.accent : 'transparent'}`,
            color: tab === t.id ? c.textPrimary : c.textMuted,
            marginBottom: -1,
          }}>{t.label}</button>
        ))}
      </div>

      {/* Content */}
      <div style={{ flex: 1, overflow: 'auto' }}>
        {tab === 'messages' && (
          <div style={{ display: 'flex', flexDirection: 'column' }}>
            {(swarm.messages ?? []).length === 0
              ? <Empty c={c} text="No messages yet" />
              : (swarm.messages ?? []).map((msg, i) => (
                <div key={i} style={{ padding: '12px 16px', borderBottom: `1px solid ${c.border}` }}>
                  <div style={{ display: 'flex', justifyContent: 'space-between', marginBottom: 5 }}>
                    <span style={{ fontSize: 11, fontWeight: 600 }}>{msg.from}</span>
                    <span style={{ fontSize: 9, fontFamily: MONO, color: c.textMuted }}>{relativeTime(msg.timestamp)}</span>
                  </div>
                  <div style={{ fontSize: 12, color: c.textSecondary, lineHeight: 1.6 }}>
                    {String(msg.content?.text ?? '').slice(0, 200)}
                  </div>
                </div>
              ))
            }
          </div>
        )}

        {tab === 'events' && (
          <div style={{ display: 'flex', flexDirection: 'column' }}>
            <div style={{ padding: '8px 14px', borderBottom: `1px solid ${c.border}`, display: 'flex', gap: 8 }}>
              <button onClick={onToggleStream} style={{ fontSize: 10, fontFamily: MONO, padding: '3px 10px', cursor: 'pointer',
                background: streaming ? 'rgba(34,197,94,0.1)' : 'transparent',
                color: streaming ? c.success : c.textMuted, border: `1px solid ${streaming ? c.success + '40' : c.border}` }}>
                {streaming ? '⏹ Stop' : '▶ Stream'}
              </button>
            </div>
            {liveEvents.length === 0
              ? <Empty c={c} text={streaming ? 'Waiting for events…' : 'Start stream to see live events'} />
              : liveEvents.map((ev, i) => {
                  const color = EVENT_TYPE_COLOR[ev.event] ?? c.textMuted;
                  return (
                    <div key={i} style={{ padding: '10px 14px', borderBottom: `1px solid ${c.border}`,
                      display: 'grid', gridTemplateColumns: 'auto auto 1fr', gap: 10, alignItems: 'center', opacity: Math.max(0.3, 1 - i * 0.05) }}>
                      <span style={{ width: 6, height: 6, background: color }} />
                      <span style={{ fontSize: 9, fontFamily: MONO, color: c.textMuted, whiteSpace: 'nowrap' }}>{ev.event}</span>
                      <span style={{ fontSize: 11, color: c.textSecondary, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                        {String((ev.data as Record<string, unknown>)?.agentName ?? '')}
                      </span>
                    </div>
                  );
                })
            }
          </div>
        )}

        {tab === 'results' && (
          <div style={{ display: 'flex', flexDirection: 'column' }}>
            {(swarm.results ?? []).length === 0
              ? <Empty c={c} text="No results yet" />
              : (swarm.results ?? []).map((r, i) => (
                <div key={i} style={{ padding: '14px 16px', borderBottom: `1px solid ${c.border}` }}>
                  <div style={{ display: 'flex', justifyContent: 'space-between', marginBottom: 6 }}>
                    <StatusBadge status={r.status} dark={dark} c={c} />
                    {r.durationMs !== undefined && (
                      <span style={{ fontSize: 10, fontFamily: MONO, color: c.textMuted }}>{(r.durationMs / 1000).toFixed(2)}s</span>
                    )}
                  </div>
                  {r.data && (
                    <pre style={{ margin: 0, fontSize: 11, fontFamily: MONO, color: c.textSecondary, whiteSpace: 'pre-wrap', lineHeight: 1.5 }}>
                      {JSON.stringify(r.data, null, 2).slice(0, 300)}
                    </pre>
                  )}
                  {r.error && <div style={{ fontSize: 12, color: c.danger, marginTop: 4 }}>{r.error}</div>}
                </div>
              ))
            }
          </div>
        )}
      </div>
    </div>
  );
}

function Empty({ c, text }: { c: Colors; text: string }) {
  return <div style={{ padding: '32px 16px', textAlign: 'center', fontSize: 12, fontFamily: MONO, color: c.textMuted }}>{text}</div>;
}

function EmptyState({ c, label, sub }: { c: Colors; label: string; sub?: string }) {
  return (
    <div style={{ flex: 1, display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center', gap: 8, padding: 40 }}>
      <div style={{ fontSize: 16, fontWeight: 600, color: c.textMuted }}>{label}</div>
      {sub && <div style={{ fontSize: 12, color: c.textMuted, fontFamily: MONO }}>{sub}</div>}
    </div>
  );
}
