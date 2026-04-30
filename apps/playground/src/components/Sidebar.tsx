import { useMemo, useState } from 'react';
import type { AgentSnapshot, SwarmState } from '../lib/api';

export type EntityKind = 'agents' | 'swarms' | 'agencies';
export type EntityRow = {
  id: string;
  name: string;
  status: string;
  tokens: number;
  sub?: string;
};

interface Props {
  kind: EntityKind;
  onKindChange: (k: EntityKind) => void;
  agents: AgentSnapshot[];
  swarms: SwarmState[];
  selectedId: string | null;
  onSelect: (id: string | null) => void;
  onNew: () => void;
  onDelete: (id: string) => void;
}

export function Sidebar({
  kind,
  onKindChange,
  agents,
  swarms,
  selectedId,
  onSelect,
  onNew,
  onDelete,
}: Props) {
  const [filter, setFilter] = useState('');

  const rows = useMemo<EntityRow[]>(() => {
    let all: EntityRow[] = [];
    if (kind === 'agents') {
      all = agents.map((a) => ({
        id: a.state.id,
        name: a.state.name,
        status: a.state.status,
        tokens: a.state.tokenUsage.totalTokens,
        sub: a.state.config?.model,
      }));
    } else if (kind === 'swarms') {
      all = swarms.map((s) => ({
        id: s.id,
        name: `swarm.${s.id.slice(0, 8)}`,
        status: s.status,
        tokens: s.tokenUsage.totalTokens,
        sub: `${s.agentIds.length} agents`,
      }));
    }

    const q = filter.trim().toLowerCase();
    if (!q) return all;
    return all.filter(
      (r) =>
        r.name.toLowerCase().includes(q) || r.id.toLowerCase().includes(q)
    );
  }, [kind, agents, swarms, filter]);

  return (
    <aside className="flex flex-col bg-[var(--surface)] border-r border-[var(--border)] overflow-hidden">
      <div className="flex p-1 m-3 mb-2 bg-[var(--surface-2)] rounded-lg border border-[var(--border)]">
        {(['agents', 'swarms', 'agencies'] as const).map((k) => (
          <button
            key={k}
            onClick={() => onKindChange(k)}
            className={`flex-1 px-3 py-1.5 text-sm rounded-md capitalize transition-colors ${
              kind === k
                ? 'bg-[var(--surface-3)] text-[var(--text)] shadow-sm'
                : 'text-[var(--muted)] hover:text-[var(--text-2)]'
            }`}
          >
            {k}
          </button>
        ))}
      </div>

      <div className="px-3 pb-3 flex items-center gap-2">
        <input
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          placeholder="Search…"
          className="text-sm"
        />
        <button
          onClick={onNew}
          title="New"
          className="shrink-0 px-3 py-2 text-sm rounded-md bg-[var(--accent)] text-[var(--accent-fg)] hover:bg-[var(--accent-hover)] transition-colors"
        >
          + New
        </button>
      </div>

      <div className="flex-1 overflow-y-auto px-2 pb-3">
        {rows.length === 0 && (
          <div className="px-3 py-6 text-sm text-[var(--muted)] text-center">
            {filter
              ? 'No matches.'
              : kind === 'agencies'
              ? 'No agency workspace drafts in this panel yet. Click + New to create one from a prompt.'
              : `No ${kind} yet. Click + New.`}
          </div>
        )}
        <div className="flex flex-col gap-0.5">
          {rows.map((r) => {
            const selected = r.id === selectedId;
            return (
              <div
                key={r.id}
                onClick={() => onSelect(r.id)}
                className={`group relative px-3 py-2.5 rounded-md cursor-pointer transition-colors ${
                  selected
                    ? 'bg-[var(--accent-soft)]'
                    : 'hover:bg-[var(--hover)]'
                }`}
              >
                <div className="flex items-baseline justify-between gap-2">
                  <span
                    className={`truncate font-medium ${
                      selected ? 'text-[var(--text)]' : 'text-[var(--text-2)]'
                    }`}
                  >
                    {r.name}
                  </span>
                  <span className="text-xs text-[var(--muted-2)] tabular-nums shrink-0">
                    {r.tokens.toLocaleString()}
                  </span>
                </div>
                <div className="flex items-center justify-between gap-2 mt-0.5">
                  <span className="text-xs text-[var(--muted)] flex items-center gap-1.5 truncate">
                    <StatusDot status={r.status} />
                    <span className="capitalize">{r.status}</span>
                    {r.sub && <span className="truncate">· {r.sub}</span>}
                  </span>
                  <button
                    onClick={(e) => {
                      e.stopPropagation();
                      if (confirm(`Delete ${r.name}?`)) onDelete(r.id);
                    }}
                    className="opacity-0 group-hover:opacity-100 text-xs text-[var(--muted)] hover:text-[var(--err)] transition-opacity shrink-0"
                  >
                    Delete
                  </button>
                </div>
                <div className="text-[10px] font-[var(--mono)] text-[var(--muted-2)] mt-0.5 truncate">
                  {r.id.slice(0, 8)}
                </div>
              </div>
            );
          })}
        </div>
      </div>

      <div className="px-3 py-2 border-t border-[var(--border)] text-xs text-[var(--muted-2)]">
        {kind === 'agencies' ? 'Prompt-based workspace creation' : `${rows.length} ${kind}`}
      </div>
    </aside>
  );
}

function StatusDot({ status }: { status: string }) {
  const color =
    status === 'idle' || status === 'completed' || status === 'ok'
      ? 'var(--ok)'
      : status === 'running'
      ? 'var(--warn)'
      : status === 'error' || status === 'failed'
      ? 'var(--err)'
      : 'var(--muted)';
  return (
    <span
      className="inline-block w-1.5 h-1.5 rounded-full shrink-0"
      style={{ background: color }}
    />
  );
}
