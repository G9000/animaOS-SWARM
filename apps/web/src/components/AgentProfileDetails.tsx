import { useEffect, useState } from 'react';
import type { AgentMemory } from '@animaOS-SWARM/sdk';
import { daemon } from '../lib/daemon-api';
import type { AgentDetail } from '../lib/types';
import { AgentAvatar, refreshAgentAvatar } from './AgentAvatar';

export function AgentProfileDetails({
  agent,
  disabled,
}: {
  agent: AgentDetail;
  disabled: boolean;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState('');
  const change = async (file: File | null) => {
    setError(null);
    setNotice('');
    if (
      file &&
      (!['image/png', 'image/jpeg', 'image/webp'].includes(file.type) ||
        file.size > 5 * 1024 * 1024)
    ) {
      setError('Choose a PNG, JPEG, or WebP image up to 5 MB.');
      return;
    }
    setBusy(true);
    try {
      if (file) await daemon.setAgentAvatar(agent.id, file);
      else await daemon.removeAgentAvatar(agent.id);
      refreshAgentAvatar(agent.id);
      setNotice(file ? 'Avatar saved.' : 'Avatar removed.');
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="space-y-4" aria-label="Agent identity">
      <div className="flex items-center gap-4">
        <AgentAvatar id={agent.id} name={agent.name} size={64} />
        <div className="min-w-0">
          <p className="break-words text-xl font-semibold">{agent.name}</p>
          <p className="mt-1 text-sm text-ink-3">
            {agent.workspaceRole === 'lead' ? 'Workspace manager' : 'Teammate'}{' '}
            ·{' '}
            {agent.status === 'Running'
              ? 'Working now'
              : agent.status === 'Failed'
                ? 'Needs attention'
                : agent.status === 'Terminated'
                  ? 'Stopped'
                  : 'Ready for a conversation'}
          </p>
        </div>
      </div>
      <label className="block text-xs text-ink-3">
        Change avatar
        <input
          aria-label="Change agent avatar"
          type="file"
          accept="image/png,image/jpeg,image/webp"
          disabled={disabled || busy}
          className="mt-2 block w-full text-sm"
          onChange={(event) => {
            const file = event.target.files?.[0];
            event.target.value = '';
            if (file) void change(file);
          }}
        />
      </label>
      <div className="flex items-center justify-between gap-3">
        <span className="text-xs text-ink-3">
          PNG, JPEG or WebP · up to 5 MB
        </span>
        <button
          type="button"
          className="text-xs underline"
          disabled={disabled || busy}
          onClick={() => void change(null)}
        >
          Remove avatar
        </button>
      </div>
      {busy && (
        <p role="status" className="text-xs">
          Saving avatar…
        </p>
      )}
      {notice && (
        <p role="status" className="text-xs">
          {notice}
        </p>
      )}
      {error && (
        <p role="alert" className="text-sm text-danger">
          {error}
        </p>
      )}
    </section>
  );
}

export function AgentMemoryView({ agentId }: { agentId: string }) {
  const [memories, setMemories] = useState<AgentMemory[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState('');
  const [revision, setRevision] = useState(0);
  useEffect(() => {
    let cancelled = false;
    setMemories(null);
    setError(null);
    daemon
      .recentAgentMemories(agentId)
      .then((result) => {
        if (!cancelled) setMemories(result.memories);
      })
      .catch((caught) => {
        if (!cancelled)
          setError(caught instanceof Error ? caught.message : String(caught));
      });
    return () => {
      cancelled = true;
    };
  }, [agentId, revision]);
  const filtered = memories?.filter((item) =>
    `${item.content} ${item.tags?.join(' ') ?? ''}`
      .toLowerCase()
      .includes(query.toLowerCase()),
  );
  return (
    <section className="space-y-4" aria-label="Agent memory">
      <div className="flex items-center justify-between">
        <h4 className="font-semibold">Memory</h4>
        <button
          type="button"
          className="text-xs underline"
          onClick={() => setRevision((value) => value + 1)}
        >
          Refresh memory
        </button>
      </div>
      <p className="text-sm leading-relaxed text-ink-3">
        The latest 100 memories stored for this agent, including their scope and
        when they were recorded.
      </p>
      <input
        className="field"
        aria-label="Search agent memory"
        placeholder="Search these memories…"
        value={query}
        onChange={(event) => setQuery(event.target.value)}
      />
      {error ? (
        <p role="alert" className="text-sm text-danger">
          {error}
        </p>
      ) : memories === null ? (
        <p role="status">Loading memories…</p>
      ) : memories.length === 0 ? (
        <p className="text-sm text-ink-3">
          No saved memories yet. Memories will appear here when the agent
          records them.
        </p>
      ) : filtered?.length === 0 ? (
        <p className="text-sm text-ink-3">No matching memories.</p>
      ) : (
        filtered?.map((memory) => (
          <article
            key={memory.id}
            className="space-y-3 rounded-xl border border-line p-4"
          >
            <div className="flex flex-wrap gap-2 text-xs text-ink-3">
              <span>{memory.scope}</span>
              <span>· {memory.type}</span>
              <time>{new Date(memory.createdAt).toLocaleString()}</time>
            </div>
            <p className="whitespace-pre-wrap break-words text-sm leading-relaxed">
              {memory.content}
            </p>
            {!!memory.tags?.length && (
              <p className="break-words text-xs text-ink-3">
                {memory.tags.join(' · ')}
              </p>
            )}
          </article>
        ))
      )}
    </section>
  );
}
