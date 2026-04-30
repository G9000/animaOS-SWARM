import { useCallback, useEffect, useState } from 'react';
import {
  agents,
  providers as providersApi,
  swarms,
  type AgentSnapshot,
  type Provider,
  type SwarmState,
} from './lib/api';
import { HealthBadge } from './components/HealthBadge';
import { Sidebar, type EntityKind } from './components/Sidebar';
import { Chat } from './components/Chat';
import { CreateAgent } from './components/CreateAgent';
import { CreateAgency } from './components/CreateAgency';
import { CreateSwarm } from './components/CreateSwarm';
import { MemoryInspector } from './components/MemoryInspector';

type View = 'chat' | 'create' | 'empty';

export function Playground() {
  const [kind, setKind] = useState<EntityKind>('agents');
  const [agentList, setAgentList] = useState<AgentSnapshot[]>([]);
  const [swarmList, setSwarmList] = useState<SwarmState[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [view, setView] = useState<View>('empty');
  const [loadError, setLoadError] = useState<string | null>(null);
  const [providerList, setProviderList] = useState<Provider[] | null>(null);
  const [memoryRefreshKey, setMemoryRefreshKey] = useState(0);

  const refreshAgents = useCallback(async () => {
    try {
      const list = await agents.list();
      setAgentList(list);
      setLoadError(null);
    } catch (e) {
      setLoadError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  const refreshSwarms = useCallback(async () => {
    try {
      const list = await swarms.list();
      setSwarmList(list);
      setLoadError(null);
    } catch (e) {
      setLoadError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  const refreshActive = useCallback(async () => {
    if (kind === 'agents') return refreshAgents();
    if (kind === 'swarms') return refreshSwarms();
    return Promise.resolve();
  }, [kind, refreshAgents, refreshSwarms]);

  useEffect(() => {
    refreshAgents();
    refreshSwarms();
    const id = setInterval(() => {
      refreshAgents();
      refreshSwarms();
    }, 5000);
    return () => clearInterval(id);
  }, [refreshAgents, refreshSwarms]);

  useEffect(() => {
    let cancelled = false;
    const tick = async () => {
      try {
        const list = await providersApi.list();
        if (!cancelled) setProviderList(list);
      } catch {
        // surfaced via dropdown placeholder
      }
    };
    tick();
    const id = setInterval(tick, 30000);
    return () => {
      cancelled = true;
      clearInterval(id);
    };
  }, []);

  const selected =
    selectedId == null
      ? null
      : kind === 'agents'
      ? agentList.find((a) => a.state.id === selectedId) ?? null
      : kind === 'swarms'
      ? swarmList.find((s) => s.id === selectedId) ?? null
      : null;

  useEffect(() => {
    if (selectedId == null) return;
    const exists =
      kind === 'agents'
        ? agentList.some((a) => a.state.id === selectedId)
        : kind === 'swarms'
        ? swarmList.some((s) => s.id === selectedId)
        : false;
    if (!exists) {
      setSelectedId(null);
      setView('empty');
    }
  }, [kind, agentList, swarmList, selectedId]);

  function handleKindChange(next: EntityKind) {
    setKind(next);
    setSelectedId(null);
    setView(next === 'agencies' ? 'create' : 'empty');
  }

  function handleSelect(id: string | null) {
    setSelectedId(id);
    setView(id ? 'chat' : 'empty');
  }

  function handleNew() {
    setSelectedId(null);
    setView('create');
  }

  async function handleDelete(id: string) {
    if (kind === 'agencies') {
      alert('Agency workspaces are written to disk and are not listed by the daemon yet.');
      return;
    }
    if (kind === 'swarms') {
      alert('Swarm deletion is not exposed by the daemon yet.');
      return;
    }
    try {
      await agents.delete(id);
      if (selectedId === id) {
        setSelectedId(null);
        setView('empty');
      }
      await refreshAgents();
    } catch (e) {
      alert(e instanceof Error ? e.message : String(e));
    }
  }

  async function handleCreated(id: string) {
    await refreshActive();
    setSelectedId(id);
    setView('chat');
  }

  async function handleAgencySpawned(id: string) {
    await refreshSwarms();
    setKind('swarms');
    setSelectedId(id);
    setView('chat');
  }

  async function handleAfterRun() {
    await refreshActive();
    setMemoryRefreshKey((value) => value + 1);
  }

  return (
    <div className="grid grid-rows-[auto_1fr] grid-cols-1 h-screen">
      <header className="flex items-center gap-3 px-5 h-14 border-b border-[var(--border)] bg-[var(--surface)]">
        <div className="flex items-center gap-2">
          <div className="w-6 h-6 rounded-md bg-[var(--accent)] flex items-center justify-center text-[var(--accent-fg)] text-xs font-semibold">
            a
          </div>
          <span className="font-semibold text-[var(--text)]">animaOS</span>
          <span className="text-[var(--muted-2)]">/</span>
          <span className="text-[var(--text-2)]">Playground</span>
        </div>
        <span className="hidden md:inline text-xs text-[var(--muted-2)] ml-2">
          Dev tool · talks to the daemon on :8080
        </span>
        <div className="ml-auto flex items-center gap-3">
          {loadError && (
            <span className="text-xs text-[var(--err)]" title={loadError}>
              Connection error
            </span>
          )}
          <HealthBadge />
          <a
            href="/docs"
            target="_blank"
            rel="noreferrer"
            className="text-sm text-[var(--muted)] hover:text-[var(--text)] transition-colors"
          >
            API docs ↗
          </a>
        </div>
      </header>

      <main className="grid grid-cols-[280px_1fr] min-h-0">
        <Sidebar
          kind={kind}
          onKindChange={handleKindChange}
          agents={agentList}
          swarms={swarmList}
          selectedId={selectedId}
          onSelect={handleSelect}
          onNew={handleNew}
          onDelete={handleDelete}
        />

        <section className="min-w-0 min-h-0 grid grid-cols-1 xl:grid-cols-[minmax(0,1fr)_380px] overflow-hidden bg-[var(--bg)]">
          <div className="min-w-0 min-h-0 overflow-hidden">
            {view === 'empty' && (
              <EmptyState
                kind={kind}
                hasItems={
                  kind === 'agencies'
                    ? false
                    : kind === 'agents'
                    ? agentList.length > 0
                    : swarmList.length > 0
                }
                onNew={handleNew}
              />
            )}
            {view === 'create' && kind === 'agents' && (
              <div className="h-full overflow-y-auto">
                <CreateAgent
                  onCreated={handleCreated}
                  onCancel={() => setView('empty')}
                  providers={providerList}
                />
              </div>
            )}
            {view === 'create' && kind === 'swarms' && (
              <div className="h-full overflow-y-auto">
                <CreateSwarm
                  onCreated={handleCreated}
                  onCancel={() => setView('empty')}
                  providers={providerList}
                />
              </div>
            )}
            {view === 'create' && kind === 'agencies' && (
              <CreateAgency
                onCancel={() => setView('empty')}
                onSwarmCreated={handleAgencySpawned}
                providers={providerList}
              />
            )}
            {view === 'chat' && kind !== 'agencies' && selected && (
              <Chat
                key={selectedId ?? ''}
                kind={kind}
                entity={selected}
                onAfterRun={handleAfterRun}
              />
            )}
          </div>
          <MemoryInspector
            agents={agentList}
            selectedAgentId={kind === 'agents' ? selectedId : null}
            refreshKey={memoryRefreshKey}
          />
        </section>
      </main>
    </div>
  );
}

function EmptyState({
  kind,
  hasItems,
  onNew,
}: {
  kind: EntityKind;
  hasItems: boolean;
  onNew: () => void;
}) {
  const noun =
    kind === 'agents' ? 'agent' : kind === 'swarms' ? 'swarm' : 'agency';
  return (
    <div className="h-full flex items-center justify-center px-6">
      <div className="text-center max-w-sm flex flex-col gap-3">
        <h2 className="text-xl font-semibold text-[var(--text)] m-0">
          {hasItems ? `Pick an ${noun}` : `Create your first ${noun}`}
        </h2>
        <p className="text-sm text-[var(--muted)] m-0">
          {hasItems
            ? `Select a ${noun} from the sidebar to start chatting, or create a new one.`
            : kind === 'agents'
            ? 'Agents are individual conversational endpoints backed by a model provider.'
            : kind === 'agencies'
            ? 'Agencies are prompt-generated workspace bundles, similar to `animaos create`, and can optionally be spawned as live swarms.'
            : 'Swarms wrap a manager and one or more workers under a coordination strategy.'}
        </p>
        <div className="flex justify-center gap-2 mt-2">
          <button
            onClick={onNew}
            className="px-4 py-2 text-sm font-medium rounded-md bg-[var(--accent)] text-[var(--accent-fg)] hover:bg-[var(--accent-hover)] transition-colors"
          >
            + New {noun}
          </button>
        </div>
      </div>
    </div>
  );
}
