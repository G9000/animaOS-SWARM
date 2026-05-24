import React, { useState } from 'react';
import { Memory, MemorySearchResult, memories as memoriesApi } from './lib/api';
import { Colors, MONO, relativeTime, avatarUrl } from './design';
import { AgentAvatar, FilterPills, SearchInput } from './ui';

interface Props {
  recentMemories: Memory[];
  dark: boolean;
  c: Colors;
}

const MEM_TYPES  = ['all', 'fact', 'observation', 'task_result', 'reflection'] as const;
const MEM_SCOPES = ['all', 'shared', 'private', 'room'] as const;

const TYPE_STYLE = (type: string, dark: boolean) => {
  const map: Record<string, { bg: string; color: string }> = {
    fact:        { bg: dark ? 'rgba(56,189,248,0.10)'  : 'rgba(14,165,233,0.08)',   color: '#0ea5e9' },
    knowledge:   { bg: dark ? 'rgba(167,139,250,0.10)' : 'rgba(124,58,237,0.08)',   color: '#8b5cf6' },
    observation: { bg: dark ? 'rgba(167,139,250,0.10)' : 'rgba(124,58,237,0.08)',   color: '#8b5cf6' },
    reflection:  { bg: dark ? 'rgba(251,191,36,0.10)'  : 'rgba(180,83,9,0.08)',     color: '#f59e0b' },
    task_result: { bg: dark ? 'rgba(34,197,94,0.10)'   : 'rgba(22,163,74,0.08)',    color: '#22c55e' },
  };
  return map[type] ?? map.fact;
};

export function ViewMemory({ recentMemories, dark, c }: Props) {
  const [memories, setMemories]   = useState<Memory[]>(recentMemories);
  const [search, setSearch]       = useState('');
  const [typeFilter, setType]     = useState('all');
  const [scopeFilter, setScope]   = useState('all');
  const [agentFilter, setAgent]   = useState('all');
  const [sort, setSort]           = useState<'importance' | 'recent'>('importance');
  const [searching, setSearching] = useState(false);
  const [searchResults, setSearchResults] = useState<MemorySearchResult[] | null>(null);

  const agentNames = [...new Set(memories.map(m => m.agentName))].sort();

  async function handleSearch() {
    if (!search.trim()) { setSearchResults(null); return; }
    setSearching(true);
    try {
      const results = await memoriesApi.search({ q: search.trim(), limit: 24 });
      setSearchResults(results);
    } catch { /* ignore */ } finally { setSearching(false); }
  }

  const source: Memory[] = searchResults ?? memories;

  const filtered = source.filter(m => {
    if (typeFilter !== 'all' && m.type !== typeFilter) return false;
    if (scopeFilter !== 'all' && m.scope !== scopeFilter) return false;
    if (agentFilter !== 'all' && m.agentName !== agentFilter) return false;
    return true;
  }).sort((a, b) => sort === 'importance' ? b.importance - a.importance : b.createdAt - a.createdAt);

  const counts = {
    fact:        memories.filter(m => m.type === 'fact').length,
    observation: memories.filter(m => m.type === 'observation').length,
    reflection:  memories.filter(m => m.type === 'reflection').length,
    task_result: memories.filter(m => m.type === 'task_result').length,
  };

  return (
    <div style={{ flex: 1, display: 'flex', flexDirection: 'column', minHeight: 0 }}>
      {/* Header */}
      <div style={{ padding: '20px 28px', borderBottom: `1px solid ${c.border}`, flexShrink: 0 }}>
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start', marginBottom: 16 }}>
          <div>
            <div style={{ fontWeight: 700, fontSize: 20, letterSpacing: -0.4 }}>Memory</div>
            <div style={{ fontSize: 11, color: c.textMuted, marginTop: 2, fontFamily: MONO }}>
              {filtered.length} of {memories.length} records{searchResults ? ' · search results' : ' · recent'}
            </div>
          </div>
          {/* Type counts */}
          <div style={{ display: 'flex', border: `1px solid ${c.border}` }}>
            {Object.entries(counts).map(([k, v], i) => (
              <div key={k} style={{ padding: '8px 14px', borderRight: i < 3 ? `1px solid ${c.border}` : 'none', textAlign: 'center', minWidth: 60 }}>
                <div style={{ fontSize: 9, fontFamily: MONO, letterSpacing: 1, textTransform: 'uppercase', color: TYPE_STYLE(k, dark).color }}>{k}</div>
                <div style={{ fontSize: 16, fontWeight: 700, marginTop: 2 }}>{v}</div>
              </div>
            ))}
          </div>
        </div>

        {/* Filters + semantic search */}
        <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap', alignItems: 'center' }}>
          <div style={{ display: 'flex', flex: 1, maxWidth: 400 }}>
            <div style={{ position: 'relative', flex: 1 }}>
              <span style={{ position: 'absolute', left: 10, top: '50%', transform: 'translateY(-50%)', color: c.textMuted, fontSize: 14 }}>⌕</span>
              <input value={search} onChange={e => { setSearch(e.target.value); if (!e.target.value) setSearchResults(null); }}
                onKeyDown={e => e.key === 'Enter' && handleSearch()}
                placeholder="Semantic search…"
                style={{ width: '100%', padding: '8px 10px 8px 30px', background: c.elevated, color: c.textPrimary, border: `1px solid ${c.border}`, outline: 'none', fontSize: 12, fontFamily: 'inherit' }} />
            </div>
            <button onClick={handleSearch} disabled={searching}
              style={{ padding: '8px 14px', fontSize: 11, fontFamily: MONO, background: c.accent, color: dark ? '#0f1115' : '#fff', border: 'none', cursor: 'pointer', flexShrink: 0 }}>
              {searching ? '…' : 'Search'}
            </button>
          </div>

          <FilterPills options={['all', 'fact', 'reflection']} value={typeFilter} onChange={setType} c={c} dark={dark} />
          <FilterPills options={['all', 'shared', 'private']} value={scopeFilter} onChange={setScope} c={c} dark={dark} />

          <select value={agentFilter} onChange={e => setAgent(e.target.value)}
            style={{ padding: '7px 12px', background: c.elevated, color: c.textPrimary, border: `1px solid ${c.border}`, outline: 'none', fontSize: 12, fontFamily: MONO, cursor: 'pointer' }}>
            <option value="all">All agents</option>
            {agentNames.map(n => <option key={n} value={n}>{n}</option>)}
          </select>

          <select value={sort} onChange={e => setSort(e.target.value as 'importance' | 'recent')}
            style={{ padding: '7px 12px', background: c.elevated, color: c.textPrimary, border: `1px solid ${c.border}`, outline: 'none', fontSize: 12, fontFamily: MONO, cursor: 'pointer' }}>
            <option value="importance">↓ Importance</option>
            <option value="recent">↓ Recent</option>
          </select>
        </div>
      </div>

      {/* Grid */}
      <div style={{ flex: 1, overflow: 'auto', padding: '20px 28px' }}>
        {filtered.length === 0 ? (
          <div style={{ textAlign: 'center', padding: '60px 0', color: c.textMuted, fontFamily: MONO, fontSize: 12 }}>
            No memories match · try a different filter
          </div>
        ) : (
          <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(300px, 1fr))', gap: 12 }}>
            {filtered.map(mem => (
              <MemCard key={mem.id} mem={mem} dark={dark} c={c}
                score={(mem as MemorySearchResult).score}
              />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

// ── Memory card ───────────────────────────────────────────────────────────────
function MemCard({ mem, dark, c, score }: { mem: Memory; dark: boolean; c: Colors; score?: number }) {
  const [hover, setHover] = useState(false);
  const ts = TYPE_STYLE(mem.type, dark);

  return (
    <div onMouseEnter={() => setHover(true)} onMouseLeave={() => setHover(false)}
      style={{
        border: `1px solid ${hover ? c.borderStrong : c.border}`,
        borderTop: `2px solid ${ts.color}`,
        background: c.elevated,
        padding: '14px 16px', display: 'flex', flexDirection: 'column', gap: 10,
        transition: 'border-color 0.12s',
      }}>
      {/* Top row */}
      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start', gap: 8 }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 7 }}>
          <AgentAvatar name={mem.agentName} size={22} c={c} />
          <span style={{ fontSize: 11, fontWeight: 600 }}>{mem.agentName}</span>
        </div>
        <div style={{ display: 'flex', gap: 5, flexShrink: 0 }}>
          <span style={{ fontSize: 9, padding: '2px 7px', background: ts.bg, color: ts.color, fontFamily: MONO }}>{mem.type}</span>
          <span style={{ fontSize: 9, padding: '2px 7px', border: `1px solid ${c.border}`, color: c.textMuted, fontFamily: MONO }}>{mem.scope}</span>
        </div>
      </div>

      {/* Content */}
      <div style={{ fontSize: 12, color: c.textSecondary, lineHeight: 1.65, flex: 1 }}>{mem.content}</div>

      {/* Importance bar */}
      <div>
        <div style={{ display: 'flex', justifyContent: 'space-between', fontSize: 9, color: c.textMuted, fontFamily: MONO, marginBottom: 4 }}>
          <span>{score !== undefined ? 'SCORE' : 'IMPORTANCE'}</span>
          <span>{((score ?? mem.importance) * 100).toFixed(0)}%</span>
        </div>
        <div style={{ height: 3, background: c.border }}>
          <div style={{ width: `${(score ?? mem.importance) * 100}%`, height: '100%', transition: 'width 0.4s',
            background: mem.importance > 0.9 ? c.success : mem.importance > 0.75 ? c.accent : c.textMuted }} />
        </div>
      </div>

      {/* Tags + time */}
      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-end', gap: 8 }}>
        <div style={{ display: 'flex', gap: 4, flexWrap: 'wrap' }}>
          {(mem.tags ?? []).map(t => (
            <span key={t} style={{ fontSize: 9, padding: '1px 6px', background: c.subtle, color: c.textMuted, border: `1px solid ${c.border}`, fontFamily: MONO }}>#{t}</span>
          ))}
        </div>
        <span style={{ fontSize: 9, color: c.textMuted, fontFamily: MONO, flexShrink: 0 }}>{relativeTime(mem.createdAt)}</span>
      </div>
    </div>
  );
}
