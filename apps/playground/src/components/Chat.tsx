import { useCallback, useEffect, useRef, useState } from 'react';
import type { Ref } from 'react';
import {
  agents,
  memories,
  swarms,
  coerceText,
  type AgentRelationship,
  type AgentSnapshot,
  type AgentTranscriptMessage,
  type AgentMessage,
  type MemoryRecallResult,
  type SwarmMessageEventPayload,
  type SwarmState,
  type SwarmStreamEvent,
  type SwarmToolEventPayload,
  type TaskResult,
} from '../lib/api';
import { playgroundUserId, playgroundUserMetadata } from '../lib/playgroundUser';

type Kind = 'agents' | 'swarms';
type Msg = {
  id: string;
  role: 'user' | 'agent' | 'error';
  text: string;
  meta?: string;
};

type SwarmActivity = {
  id: string;
  tone: 'status' | 'message' | 'tool' | 'error';
  title: string;
  body?: string;
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
  const agentTranscript = kind === 'agents' ? (entity as AgentSnapshot).messages : [];
  const agentHistoryCount =
    kind === 'agents' ? agentTranscript.length : 0;
  const latestAgentMessageId =
    kind === 'agents' ? agentTranscript[agentTranscript.length - 1]?.id ?? '' : '';

  const [messages, setMessages] = useState<Msg[]>([]);
  const [relationships, setRelationships] = useState<AgentRelationship[]>([]);
  const [recallResults, setRecallResults] = useState<MemoryRecallResult[]>([]);
  const [relationshipsLoading, setRelationshipsLoading] = useState(false);
  const [relationshipsError, setRelationshipsError] = useState<string | null>(null);
  const [swarmActivity, setSwarmActivity] = useState<SwarmActivity[]>([]);
  const [draft, setDraft] = useState('');
  const [sending, setSending] = useState(false);
  const scrollerRef = useRef<HTMLDivElement>(null);
  const activityRef = useRef<HTMLDivElement>(null);
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
    setSwarmActivity([]);
    setDraft('');
    setTimeout(() => taRef.current?.focus(), 0);
  }, [id]);

  useEffect(() => {
    if (kind !== 'agents') return;
    setMessages(transcriptFromAgentMessages(agentTranscript));
  }, [agentHistoryCount, id, kind, latestAgentMessageId]);

  useEffect(() => {
    if (kind !== 'swarms') return;

    const history = (entity as SwarmState).messages.map((message) =>
      messageActivity(message, 'history')
    );
    setSwarmActivity((items) => mergeSwarmActivity(items, history));
  }, [entity, kind]);

  useEffect(() => {
    if (kind !== 'swarms') {
      setSwarmActivity([]);
      return;
    }

    return swarms.streamEvents(
      id,
      (event) => {
        const activity = activityFromSwarmEvent(event);
        if (!activity) return;
        setSwarmActivity((items) => mergeSwarmActivity(items, [activity]));
      },
      () => {
        setSwarmActivity((items) => [
          ...items.slice(-31),
          {
            id: `stream-error-${Date.now()}`,
            tone: 'error',
            title: 'Event stream disconnected',
            body: 'Refresh the swarm or run again to reconnect.',
          },
        ]);
      }
    );
  }, [id, kind]);

  useEffect(() => {
    refreshRelationships();
  }, [refreshRelationships]);

  useEffect(() => {
    const el = scrollerRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [messages, sending]);

  useEffect(() => {
    const el = activityRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [swarmActivity]);

  async function send() {
    const text = draft.trim();
    if (!text || sending) return;
    setMessages((m) => [...m, { id: localMessageId('user'), role: 'user', text }]);
    setDraft('');
    setSending(true);

    const t0 = performance.now();
    try {
      if (kind === 'agents') {
        const response = await agents.run(id, text, playgroundUserMetadata());
        const meta = runMeta(response.result, t0);
        const transcript = transcriptFromAgentMessages(response.agent.messages);

        setMessages(
          response.result.status === 'success'
            ? transcript
            : [
                ...transcript,
                {
                  id: localMessageId('error'),
                  role: 'error',
                  text: response.result.error || 'error',
                  meta,
                },
              ]
        );
      } else {
        const response = await swarms.run(id, text);
        const result = response.result;
        const body =
          result.status === 'success'
            ? coerceText(result.data)
            : result.error || 'error';

        setMessages((m) => [
          ...m,
          {
            id: localMessageId(result.status === 'success' ? 'agent' : 'error'),
            role: result.status === 'success' ? 'agent' : 'error',
            text: body,
            meta: runMeta(result, t0),
          },
        ]);
      }
    } catch (e) {
      setMessages((m) => [
        ...m,
        {
          id: localMessageId('error'),
          role: 'error',
          text: e instanceof Error ? e.message : String(e),
        },
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

      {kind === 'swarms' && (
        <SwarmActivityPanel
          activities={swarmActivity}
          activityRef={activityRef}
        />
      )}

      <div ref={scrollerRef} className="flex-1 overflow-y-auto">
        {messages.length === 0 ? (
          <div className="h-full flex items-center justify-center text-sm text-[var(--muted)]">
            Send a message to start the conversation.
          </div>
        ) : (
          <div className="max-w-3xl mx-auto px-6 py-8 flex flex-col gap-6">
            {messages.map((m) => (
              <Bubble key={m.id} msg={m} />
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

function SwarmActivityPanel({
  activities,
  activityRef,
}: {
  activities: SwarmActivity[];
  activityRef: Ref<HTMLDivElement>;
}) {
  return (
    <div className="border-b border-[var(--border)] bg-[var(--surface-2)]">
      <div className="px-6 py-3 flex items-center justify-between gap-3">
        <span className="text-xs font-medium uppercase tracking-wide text-[var(--muted-2)]">
          Swarm activity
        </span>
        <span className="text-[11px] tabular-nums text-[var(--muted-2)]">
          {activities.length === 0 ? 'waiting' : `${activities.length} events`}
        </span>
      </div>
      <div
        ref={activityRef}
        className="max-h-44 overflow-y-auto px-6 pb-3 flex flex-col gap-2"
      >
        {activities.length === 0 ? (
          <div className="rounded-md border border-dashed border-[var(--border)] px-3 py-2 text-xs text-[var(--muted)]">
            Stored handoffs and live swarm events will appear here.
          </div>
        ) : (
          activities.map((activity) => (
            <SwarmActivityRow key={activity.id} activity={activity} />
          ))
        )}
      </div>
    </div>
  );
}

function SwarmActivityRow({ activity }: { activity: SwarmActivity }) {
  const dotClass =
    activity.tone === 'message'
      ? 'bg-[var(--accent)]'
      : activity.tone === 'tool'
      ? 'bg-sky-500'
      : activity.tone === 'error'
      ? 'bg-[var(--err)]'
      : 'bg-[var(--muted-2)]';

  return (
    <div className="grid grid-cols-[0.5rem_1fr] gap-2 text-xs">
      <span className={`mt-1.5 h-2 w-2 rounded-full ${dotClass}`} />
      <div className="min-w-0 rounded-md border border-[var(--border)] bg-[var(--surface)] px-3 py-2">
        <div className="flex items-center justify-between gap-3">
          <span className="min-w-0 truncate font-medium text-[var(--text)]">
            {activity.title}
          </span>
          {activity.meta && (
            <span className="shrink-0 tabular-nums text-[var(--muted-2)]">
              {activity.meta}
            </span>
          )}
        </div>
        {activity.body && (
          <div className="mt-1 break-words text-[var(--text-2)]">
            {activity.body}
          </div>
        )}
      </div>
    </div>
  );
}

function activityFromSwarmEvent(event: SwarmStreamEvent): SwarmActivity | null {
  if (event.event === 'swarm:message' && isSwarmMessagePayload(event.data)) {
    return messageActivity(event.data.message);
  }

  if (event.event === 'tool:before' && isToolPayload(event.data)) {
    return {
      id: `${event.event}-${event.data.agentId}-${event.data.toolName}-${Date.now()}`,
      tone: 'tool',
      title: `${event.data.agentName} called ${event.data.toolName}`,
      body: compactJson(event.data.args),
      meta: 'tool',
    };
  }

  if (event.event === 'tool:after' && isToolPayload(event.data)) {
    return {
      id: `${event.event}-${event.data.agentId}-${event.data.toolName}-${Date.now()}`,
      tone: event.data.status === 'success' ? 'tool' : 'error',
      title: `${event.data.agentName} finished ${event.data.toolName}`,
      body: event.data.result,
      meta: [event.data.status, formatDuration(event.data.durationMs)]
        .filter(Boolean)
        .join(' · '),
    };
  }

  if (event.event === 'swarm:running') {
    return {
      id: `${event.event}-${Date.now()}`,
      tone: 'status',
      title: 'Swarm running',
      meta: 'lifecycle',
    };
  }

  if (event.event === 'swarm:completed' && isLifecyclePayload(event.data)) {
    const result = event.data.result;
    return {
      id: `${event.event}-${Date.now()}`,
      tone: result?.status === 'error' ? 'error' : 'status',
      title: 'Swarm completed',
      body: result?.status === 'error' ? result.error : coerceText(result?.data),
      meta: result?.status ?? 'done',
    };
  }

  if (event.event === 'task:failed' && isAgentPayload(event.data)) {
    return {
      id: `${event.event}-${event.data.agentId}-${Date.now()}`,
      tone: 'error',
      title: `${event.data.agentName} task failed`,
      body: event.data.error,
    };
  }

  return null;
}

function messageActivity(
  message: AgentMessage,
  meta: string = 'message'
): SwarmActivity {
  const to = message.to === 'broadcast' ? 'broadcast' : shortAgentId(message.to);
  return {
    id: message.id,
    tone: 'message',
    title: `${shortAgentId(message.from)} → ${to}`,
    body: message.content.text,
    meta,
  };
}

function mergeSwarmActivity(
  existing: SwarmActivity[],
  additions: SwarmActivity[]
): SwarmActivity[] {
  if (additions.length === 0) return existing;

  const seen = new Set(existing.map((activity) => activity.id));
  const merged = [...existing];

  for (const activity of additions) {
    if (seen.has(activity.id)) continue;
    seen.add(activity.id);
    merged.push(activity);
  }

  return merged.slice(-32);
}

function transcriptFromAgentMessages(messages: AgentTranscriptMessage[]): Msg[] {
  return messages.reduce<Msg[]>((transcript, message) => {
    if (message.role === 'user') {
      transcript.push({
        id: message.id,
        role: 'user',
        text: message.content.text,
      });
      return transcript;
    }

    if (message.role === 'assistant') {
      transcript.push({
        id: message.id,
        role: 'agent',
        text: message.content.text,
      });
    }

    return transcript;
  }, []);
}

function runMeta(result: TaskResult, startedAt: number): string {
  const dt = Math.round(performance.now() - startedAt);
  return `${dt}ms${result.durationMs ? ` · server ${result.durationMs}ms` : ''}`;
}

function localMessageId(prefix: 'user' | 'agent' | 'error'): string {
  return `${prefix}-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}

function isSwarmMessagePayload(
  value: unknown
): value is SwarmMessageEventPayload {
  return isRecord(value) && isRecord(value.message);
}

function isLifecyclePayload(value: unknown): value is { result?: TaskResult | null } {
  return isRecord(value);
}

function isAgentPayload(value: unknown): value is { agentId: string; agentName: string; error?: string } {
  return (
    isRecord(value) &&
    typeof value.agentId === 'string' &&
    typeof value.agentName === 'string'
  );
}

function isToolPayload(value: unknown): value is SwarmToolEventPayload {
  if (!isRecord(value) || !isAgentPayload(value)) return false;
  return typeof (value as Record<string, unknown>).toolName === 'string';
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

function shortAgentId(id: string): string {
  const separator = id.lastIndexOf('-');
  if (separator <= 0) return id;
  return id.slice(0, separator);
}

function compactJson(value: unknown): string | undefined {
  if (!value || (isRecord(value) && Object.keys(value).length === 0)) return undefined;
  const json = JSON.stringify(value);
  return json.length > 180 ? `${json.slice(0, 177)}...` : json;
}

function formatDuration(value: number | undefined): string | undefined {
  return typeof value === 'number' ? `${value}ms` : undefined;
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
                  relationship {Math.round(result.relationshipScore * 100)}% · temporal{' '}
                  {Math.round(result.temporalScore * 100)}% · importance{' '}
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
