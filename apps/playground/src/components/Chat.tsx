import { useCallback, useEffect, useRef, useState } from 'react';
import {
  agents,
  memories,
  swarms,
  coerceText,
  type AgentRelationship,
  type AgentSnapshot,
  type MemoryRecallResult,
  type SwarmState,
  type TaskResult,
} from '../lib/api';
import { playgroundUserId, playgroundUserMetadata } from '../lib/playgroundUser';

type Kind = 'agents' | 'swarms';
type Msg = {
  role: 'user' | 'agent' | 'error';
  text: string;
  meta?: string;
};

interface Props {
  kind: Kind;
  entity: AgentSnapshot | SwarmState;
  onAfterRun: () => void;
}

export function Chat({ kind, entity, onAfterRun }: Props) {
  const id =
    kind === 'agents' ? (entity as AgentSnapshot).state.id : (entity as SwarmState).id;
  const name =
    kind === 'agents'
      ? (entity as AgentSnapshot).state.name
      : `swarm.${(entity as SwarmState).id.slice(0, 8)}`;
  const status =
    kind === 'agents'
      ? (entity as AgentSnapshot).state.status
      : (entity as SwarmState).status;
  const tokens =
    kind === 'agents'
      ? (entity as AgentSnapshot).state.tokenUsage.totalTokens
      : (entity as SwarmState).tokenUsage.totalTokens;

  const [messages, setMessages] = useState<Msg[]>([]);
  const [relationships, setRelationships] = useState<AgentRelationship[]>([]);
  const [recallResults, setRecallResults] = useState<MemoryRecallResult[]>([]);
  const [relationshipsLoading, setRelationshipsLoading] = useState(false);
  const [relationshipsError, setRelationshipsError] = useState<string | null>(null);
  const [draft, setDraft] = useState('');
  const [sending, setSending] = useState(false);
  const scrollerRef = useRef<HTMLDivElement>(null);
  const taRef = useRef<HTMLTextAreaElement>(null);

  const refreshRelationships = useCallback(async () => {
    if (kind !== 'agents') {
      setRelationships([]);
      setRecallResults([]);
      setRelationshipsError(null);
      setRelationshipsLoading(false);
      return;
    }

    setRelationshipsLoading(true);
    try {
      const userId = playgroundUserId();
      const [list, recalled] = await Promise.all([
        memories.relationships({ agentId: id, limit: 6 }),
        memories.recall('relationship evidence probe', {
          entityId: userId,
          agentId: id,
          recentLimit: 0,
          limit: 3,
        }),
      ]);
      setRelationships(list);
      setRecallResults(recalled);
      setRelationshipsError(null);
    } catch (e) {
      setRelationshipsError(e instanceof Error ? e.message : String(e));
    } finally {
      setRelationshipsLoading(false);
    }
  }, [id, kind]);

  useEffect(() => {
    setMessages([]);
    setDraft('');
    setTimeout(() => taRef.current?.focus(), 0);
  }, [id]);

  useEffect(() => {
    refreshRelationships();
  }, [refreshRelationships]);

  useEffect(() => {
    const el = scrollerRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [messages, sending]);

  async function send() {
    const text = draft.trim();
    if (!text || sending) return;
    setMessages((m) => [...m, { role: 'user', text }]);
    setDraft('');
    setSending(true);

    const t0 = performance.now();
    try {
      const result: TaskResult =
        kind === 'agents'
          ? (await agents.run(id, text, playgroundUserMetadata())).result
          : (await swarms.run(id, text)).result;
      const dt = Math.round(performance.now() - t0);
      const body =
        result.status === 'success'
          ? coerceText(result.data)
          : result.error || 'error';
      const meta = `${dt}ms${
        result.durationMs ? ` · server ${result.durationMs}ms` : ''
      }`;
      setMessages((m) => [
        ...m,
        {
          role: result.status === 'success' ? 'agent' : 'error',
          text: body,
          meta,
        },
      ]);
    } catch (e) {
      setMessages((m) => [
        ...m,
        { role: 'error', text: e instanceof Error ? e.message : String(e) },
      ]);
    } finally {
      setSending(false);
      onAfterRun();
      refreshRelationships();
      setTimeout(() => taRef.current?.focus(), 0);
    }
  }

  function onKeyDown(e: React.KeyboardEvent<HTMLTextAreaElement>) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      send();
    }
  }

  return (
    <div className="flex flex-col h-full">
      <div className="px-6 py-4 border-b border-[var(--border)] bg-[var(--surface)]">
        <div className="flex items-baseline justify-between gap-4">
          <div>
            <h2 className="text-base font-semibold text-[var(--text)] m-0">
              {name}
            </h2>
            <div className="text-xs text-[var(--muted)] mt-0.5 flex items-center gap-2">
              <span className="capitalize">
                {kind === 'agents' ? 'Agent' : 'Swarm'}
              </span>
              <span>·</span>
              <span className="font-mono">{id.slice(0, 8)}</span>
              <span>·</span>
              <span className="capitalize">{status}</span>
              <span>·</span>
              <span>{tokens.toLocaleString()} tokens</span>
            </div>
          </div>
          <span className="text-xs text-[var(--muted-2)] hidden md:inline">
            ⏎ to send · ⇧⏎ for newline
          </span>
        </div>
      </div>

      {kind === 'agents' && (
        <RelationshipPanel
          relationships={relationships}
          recallResults={recallResults}
          loading={relationshipsLoading}
          error={relationshipsError}
        />
      )}

      <div ref={scrollerRef} className="flex-1 overflow-y-auto">
        {messages.length === 0 ? (
          <div className="h-full flex items-center justify-center text-sm text-[var(--muted)]">
            Send a message to start the conversation.
          </div>
        ) : (
          <div className="max-w-3xl mx-auto px-6 py-8 flex flex-col gap-6">
            {messages.map((m, i) => (
              <Bubble key={i} msg={m} />
            ))}
            {sending && <Typing />}
          </div>
        )}
      </div>

      <div className="border-t border-[var(--border)] bg-[var(--surface)] px-4 py-3">
        <div className="max-w-3xl mx-auto flex gap-2 items-end">
          <textarea
            ref={taRef}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={onKeyDown}
            placeholder={`Message ${name}…`}
            rows={2}
            className="flex-1"
          />
          <button
            onClick={send}
            disabled={sending || !draft.trim()}
            className="px-4 py-2 text-sm font-medium rounded-md bg-[var(--accent)] text-[var(--accent-fg)] hover:bg-[var(--accent-hover)] disabled:bg-[var(--surface-3)] disabled:text-[var(--muted-2)] disabled:cursor-not-allowed transition-colors"
          >
            {sending ? 'Sending…' : 'Send'}
          </button>
        </div>
      </div>
    </div>
  );
}

function RelationshipPanel({
  relationships,
  recallResults,
  loading,
  error,
}: {
  relationships: AgentRelationship[];
  recallResults: MemoryRecallResult[];
  loading: boolean;
  error: string | null;
}) {
  return (
    <div className="px-6 py-3 border-b border-[var(--border)] bg-[var(--surface-2)]">
      <div className="flex items-center justify-between gap-3">
        <span className="text-xs font-medium uppercase tracking-wide text-[var(--muted-2)]">
          Memory links
        </span>
        <span className="text-[11px] tabular-nums text-[var(--muted-2)]">
          {loading ? 'loading' : `${relationships.length} shown`}
        </span>
      </div>
      {error ? (
        <div className="mt-2 text-xs text-[var(--err)] truncate" title={error}>
          {error}
        </div>
      ) : relationships.length === 0 ? (
        <div className="mt-2 text-xs text-[var(--muted)]">
          No links yet.
        </div>
      ) : (
        <div className="mt-2 flex flex-wrap gap-2">
          {relationships.map((relationship) => (
            <div
              key={relationship.id}
              className="max-w-full rounded-md border border-[var(--border)] bg-[var(--surface)] px-2.5 py-1.5 text-xs text-[var(--text-2)]"
              title={relationship.summary ?? relationship.id}
            >
              <span className="text-[var(--muted)]">{relationship.sourceKind}</span>{' '}
              <span className="font-medium text-[var(--text)]">{relationship.sourceAgentName}</span>{' '}
              <span className="text-[var(--muted-2)]">→</span>{' '}
              <span className="text-[var(--muted)]">{relationship.targetKind}</span>{' '}
              <span className="font-medium text-[var(--text)]">{relationship.targetAgentName}</span>
              <span className="ml-2 text-[var(--muted-2)]">
                {relationship.relationshipType} · {Math.round(relationship.strength * 100)}%
              </span>
            </div>
          ))}
        </div>
      )}
      {!error && recallResults.length > 0 && (
        <div className="mt-3 border-t border-[var(--border)] pt-2">
          <div className="text-[11px] font-medium uppercase tracking-wide text-[var(--muted-2)]">
            Recall evidence
          </div>
          <div className="mt-2 flex flex-col gap-1.5">
            {recallResults.map((result) => (
              <div
                key={result.memory.id}
                className="min-w-0 rounded-md border border-[var(--border)] bg-[var(--surface)] px-2.5 py-1.5 text-xs text-[var(--text-2)]"
                title={result.memory.content}
              >
                <div className="flex items-center justify-between gap-3">
                  <span className="truncate font-medium text-[var(--text)]">
                    {result.memory.content}
                  </span>
                  <span className="shrink-0 tabular-nums text-[var(--muted-2)]">
                    {Math.round(result.score * 100)}%
                  </span>
                </div>
                <div className="mt-1 text-[11px] text-[var(--muted-2)]">
                  relationship {Math.round(result.relationshipScore * 100)}% · importance{' '}
                  {Math.round(result.importanceScore * 100)}%
                </div>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

function Bubble({ msg }: { msg: Msg }) {
  const isUser = msg.role === 'user';
  const isError = msg.role === 'error';
  return (
    <div
      className={`flex ${isUser ? 'justify-end' : 'justify-start'}`}
      style={{ animation: 'fade-in 180ms ease-out' }}
    >
      <div className={`max-w-[80%] flex flex-col gap-1 ${isUser ? 'items-end' : 'items-start'}`}>
        <div
          className={`px-3.5 py-2.5 rounded-lg text-sm whitespace-pre-wrap break-words ${
            isUser
              ? 'bg-[var(--accent)] text-[var(--accent-fg)]'
              : isError
              ? 'bg-[var(--err)]/10 text-[var(--err)] border border-[var(--err)]/30 font-mono text-xs'
              : 'bg-[var(--surface-2)] text-[var(--text)] border border-[var(--border)]'
          }`}
        >
          {msg.text}
        </div>
        {msg.meta && (
          <div className="text-[10px] text-[var(--muted-2)] tabular-nums px-1">
            {msg.meta}
          </div>
        )}
      </div>
    </div>
  );
}

function Typing() {
  return (
    <div
      className="flex justify-start"
      style={{ animation: 'fade-in 180ms ease-out' }}
    >
      <div className="px-3.5 py-3 rounded-lg bg-[var(--surface-2)] border border-[var(--border)] flex items-center gap-1.5">
        {[0, 1, 2].map((i) => (
          <span
            key={i}
            className="w-1.5 h-1.5 rounded-full bg-[var(--muted)]"
            style={{
              animation: `pulse-dot 1.2s ${i * 0.15}s ease-in-out infinite`,
            }}
          />
        ))}
      </div>
    </div>
  );
}
