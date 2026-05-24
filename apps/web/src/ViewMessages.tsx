import React, { useState } from 'react';
import { AgentSnapshot, AgentTranscriptMessage } from './lib/api';
import { Colors, MONO, relativeTime, avatarUrl } from './design';
import { AgentAvatar, FilterPills, SearchInput } from './ui';

interface Props {
  agents: AgentSnapshot[];
  dark: boolean;
  c: Colors;
  onNavigate: (view: string, params?: Record<string, string>) => void;
}

const ROLES = ['all', 'user', 'assistant', 'system', 'tool'] as const;

export function ViewMessages({ agents, dark, c, onNavigate }: Props) {
  const [search, setSearch]       = useState('');
  const [roleFilter, setRole]     = useState('all');
  const [agentFilter, setAgent]   = useState('all');
  const [expanded, setExpanded]   = useState<string | null>(null);

  // Flatten all messages across agents with agent context
  const allMessages = agents.flatMap(snap =>
    (snap.messages ?? []).map(msg => ({ ...msg, agentName: snap.state.name, agentStatus: snap.state.status }))
  ).sort((a, b) => b.createdAtMs - a.createdAtMs);

  const agentNames = [...new Set(allMessages.map(m => m.agentName))].sort();

  const filtered = allMessages.filter(m => {
    if (roleFilter !== 'all' && m.role !== roleFilter) return false;
    if (agentFilter !== 'all' && m.agentName !== agentFilter) return false;
    if (search) {
      const q = search.toLowerCase();
      const text = String(m.content?.text ?? '').toLowerCase();
      if (!text.includes(q) && !m.agentName.toLowerCase().includes(q)) return false;
    }
    return true;
  });

  const ROLE_COLOR: Record<string, string> = {
    user: c.accent, assistant: c.success, system: c.warn, tool: '#a78bfa',
  };

  return (
    <div style={{ flex: 1, display: 'flex', flexDirection: 'column', minHeight: 0 }}>
      {/* Header */}
      <div style={{ padding: '20px 28px', borderBottom: `1px solid ${c.border}`, flexShrink: 0 }}>
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start', marginBottom: 16 }}>
          <div>
            <div style={{ fontWeight: 700, fontSize: 20, letterSpacing: -0.4 }}>Transcript</div>
            <div style={{ fontSize: 11, color: c.textMuted, marginTop: 2, fontFamily: MONO }}>
              agent message history · {filtered.length} of {allMessages.length} messages
            </div>
          </div>
          {/* Summary strip */}
          <div style={{ display: 'flex', border: `1px solid ${c.border}` }}>
            {(['user', 'assistant', 'system', 'tool'] as const).map((role, i) => {
              const count = allMessages.filter(m => m.role === role).length;
              return (
                <div key={role} style={{ padding: '8px 14px', borderRight: i < 3 ? `1px solid ${c.border}` : 'none', textAlign: 'center', minWidth: 60 }}>
                  <div style={{ fontSize: 9, fontFamily: MONO, letterSpacing: 1, textTransform: 'uppercase', color: ROLE_COLOR[role] ?? c.textMuted }}>{role}</div>
                  <div style={{ fontSize: 16, fontWeight: 700, marginTop: 2 }}>{count}</div>
                </div>
              );
            })}
          </div>
        </div>

        {/* Filters */}
        <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap', alignItems: 'center' }}>
          <SearchInput value={search} onChange={setSearch} placeholder="Search messages…" c={c} style={{ flex: 1, maxWidth: 360 }} />
          <FilterPills options={['all', 'user', 'assistant', 'tool']} value={roleFilter} onChange={setRole} c={c} dark={dark} />
          <select value={agentFilter} onChange={e => setAgent(e.target.value)}
            style={{ padding: '7px 12px', background: c.elevated, color: c.textPrimary, border: `1px solid ${c.border}`, outline: 'none', fontSize: 12, fontFamily: MONO, cursor: 'pointer' }}>
            <option value="all">All agents</option>
            {agentNames.map(n => <option key={n} value={n}>{n}</option>)}
          </select>
        </div>
      </div>

      {/* Message list */}
      <div style={{ flex: 1, overflow: 'auto' }}>
        {filtered.length === 0 ? (
          <div style={{ padding: '60px 0', textAlign: 'center', color: c.textMuted, fontFamily: MONO, fontSize: 12 }}>
            No messages match
          </div>
        ) : (
          <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: 13 }}>
            <thead style={{ position: 'sticky', top: 0, zIndex: 1 }}>
              <tr style={{ background: c.subtle }}>
                {['', 'Agent', 'Role', 'Content', 'Time'].map((h, i) => (
                  <th key={i} style={{ padding: '10px 16px', textAlign: 'left', fontWeight: 500,
                    fontSize: 9, color: c.textMuted, borderBottom: `1px solid ${c.border}`,
                    fontFamily: MONO, letterSpacing: 1.2, textTransform: 'uppercase' }}>{h}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {filtered.map(msg => (
                <React.Fragment key={msg.id}>
                  <tr onClick={() => setExpanded(expanded === msg.id ? null : msg.id)}
                    style={{
                      borderBottom: `1px solid ${c.border}`, cursor: 'pointer',
                      background: expanded === msg.id ? c.accentLight : 'transparent',
                      borderLeft: `2px solid ${expanded === msg.id ? c.accent : 'transparent'}`,
                    }}>
                    {/* Avatar */}
                    <td style={{ padding: '10px 0 10px 12px', width: 42 }}>
                      <AgentAvatar name={msg.agentName} size={28} status={msg.agentStatus} dark={dark} c={c} />
                    </td>
                    {/* Agent name */}
                    <td style={{ padding: '10px 16px', whiteSpace: 'nowrap' }}>
                      <button onClick={e => { e.stopPropagation(); onNavigate('agent', { id: msg.agentId }); }}
                        style={{ fontWeight: 600, fontSize: 13, background: 'none', border: 'none', cursor: 'pointer', color: c.accent, padding: 0, fontFamily: 'inherit' }}>
                        {msg.agentName}
                      </button>
                    </td>
                    {/* Role */}
                    <td style={{ padding: '10px 16px' }}>
                      <span style={{
                        fontSize: 9, padding: '2px 8px', fontFamily: MONO, letterSpacing: 0.8, textTransform: 'uppercase',
                        color: ROLE_COLOR[msg.role] ?? c.textMuted,
                        background: (ROLE_COLOR[msg.role] ?? c.textMuted) + '15',
                        border: `1px solid ${(ROLE_COLOR[msg.role] ?? c.textMuted)}30`,
                      }}>{msg.role}</span>
                    </td>
                    {/* Content preview */}
                    <td style={{ padding: '10px 16px', maxWidth: 480 }}>
                      <div style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', fontSize: 12, color: c.textSecondary }}>
                        {String(msg.content?.text ?? '').slice(0, 100) || <span style={{ color: c.textMuted, fontStyle: 'italic' }}>no text</span>}
                      </div>
                    </td>
                    {/* Time */}
                    <td style={{ padding: '10px 16px', fontFamily: MONO, fontSize: 11, color: c.textMuted, whiteSpace: 'nowrap' }}>
                      {relativeTime(msg.createdAtMs)}
                    </td>
                  </tr>
                  {/* Expanded row */}
                  {expanded === msg.id && (
                    <tr style={{ background: c.accentLight, borderBottom: `1px solid ${c.border}`, borderLeft: `2px solid ${c.accent}` }}>
                      <td colSpan={5} style={{ padding: '0 16px 16px 16px' }}>
                        <div style={{ display: 'flex', gap: 14, paddingTop: 14 }}>
                          <div style={{ flex: 1 }}>
                            <div style={{ fontSize: 9, color: c.textMuted, fontFamily: MONO, letterSpacing: 1.2, textTransform: 'uppercase', marginBottom: 8 }}>Content</div>
                            <pre style={{ margin: 0, fontSize: 12, fontFamily: MONO, color: c.textSecondary, whiteSpace: 'pre-wrap', lineHeight: 1.65,
                              padding: '12px 14px', background: c.elevated, border: `1px solid ${c.border}`, maxHeight: 240, overflowY: 'auto' }}>
                              {String(msg.content?.text ?? JSON.stringify(msg.content, null, 2))}
                            </pre>
                          </div>
                          <div style={{ minWidth: 180 }}>
                            <div style={{ fontSize: 9, color: c.textMuted, fontFamily: MONO, letterSpacing: 1.2, textTransform: 'uppercase', marginBottom: 8 }}>Metadata</div>
                            <div style={{ fontSize: 11, fontFamily: MONO, display: 'flex', flexDirection: 'column', gap: 6 }}>
                              {[
                                ['id',      msg.id.slice(0, 16) + '…'],
                                ['agent',   msg.agentId.slice(0, 16) + '…'],
                                ['room',    msg.roomId?.slice(0, 16) + '…' ?? '—'],
                                ['role',    msg.role],
                                ['created', new Date(msg.createdAtMs).toLocaleTimeString()],
                              ].map(([k, v]) => (
                                <div key={k} style={{ display: 'flex', justifyContent: 'space-between', gap: 8, paddingBottom: 5, borderBottom: `1px solid ${c.border}` }}>
                                  <span style={{ color: c.textMuted }}>{k}</span>
                                  <span style={{ color: c.textPrimary }}>{v}</span>
                                </div>
                              ))}
                            </div>
                          </div>
                        </div>
                      </td>
                    </tr>
                  )}
                </React.Fragment>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}
