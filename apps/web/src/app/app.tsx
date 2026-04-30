import { useEffect, useState } from 'react';
import {
  agencies,
  health,
  agents,
  providers,
  swarms,
  type AgencyCreateResponse,
  type AgencyGenerateResponse,
  type AgentSnapshot,
  type AgentDefinitionResponse,
  type AgentConfig,
  type ProviderResponse,
  type SwarmState,
  type SwarmCreateRequest,
  type HealthResponse,
} from '../lib/api';
import styles from './app.module.css';

type SwarmStrategy = 'supervisor' | 'dynamic' | 'round-robin';

type AgencyFormState = {
  name: string;
  description: string;
  teamSize: number;
  provider: string;
  model: string;
  modelPool: string;
  outputDir: string;
  seedMemories: boolean;
  overwrite: boolean;
  strategy: SwarmStrategy;
};

const DEFAULT_FORM: AgencyFormState = {
  name: 'Northstar Studio',
  description:
    'A strategic creative agency that turns messy product and growth ideas into clear campaigns, launch plans, and messaging systems.',
  teamSize: 4,
  provider: '',
  model: '',
  modelPool: '',
  outputDir: '',
  seedMemories: false,
  overwrite: false,
  strategy: 'supervisor',
};

function usePolled<T>(
  fetcher: () => Promise<T>,
  intervalMs = 4000
): { data: T | null; error: string | null; loading: boolean } {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    const run = async () => {
      try {
        const result = await fetcher();
        if (!cancelled) { setData(result); setError(null); }
      } catch (e) {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e));
      } finally {
        if (!cancelled) setLoading(false);
      }
    };
    run();
    const id = setInterval(run, intervalMs);
    return () => { cancelled = true; clearInterval(id); };
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return { data, error, loading };
}

function StatusDot({ status }: { status: string }) {
  const cls =
    status === 'ok' || status === 'idle' || status === 'completed'
      ? styles.dotGreen
      : status === 'running'
      ? styles.dotYellow
      : styles.dotRed;
  return <span className={`${styles.dot} ${cls}`} />;
}

function HealthBadge({ h, error }: { h: HealthResponse | null; error: string | null }) {
  if (error) return <span className={styles.badge} data-status="err">daemon offline</span>;
  if (!h) return <span className={styles.badge} data-status="pending">—</span>;
  const parts = [h.status];
  if (h.version) parts.push(`v${h.version}`);
  if (h.uptime_secs !== undefined) parts.push(`up ${Math.round(h.uptime_secs / 60)}m`);
  return (
    <span className={styles.badge} data-status="ok">
      <StatusDot status="ok" />
      {parts.join(' · ')}
    </span>
  );
}

function AgentsPanel() {
  const { data, error, loading } = usePolled(() => agents.list());
  return (
    <section className={styles.panel}>
      <h2 className={styles.panelTitle}>agents</h2>
      {loading && <p className={styles.muted}>loading…</p>}
      {error && <p className={styles.err}>{error}</p>}
      {data && data.length === 0 && <p className={styles.muted}>no agents</p>}
      {data && data.length > 0 && (
        <table className={styles.table}>
          <thead>
            <tr>
              <th>name</th>
              <th>status</th>
              <th>tokens</th>
              <th>id</th>
            </tr>
          </thead>
          <tbody>
            {data.map((a: AgentSnapshot) => (
              <tr key={a.state.id}>
                <td>{a.state.name}</td>
                <td>
                  <StatusDot status={a.state.status} />
                  {a.state.status}
                </td>
                <td>{a.state.tokenUsage.totalTokens.toLocaleString()}</td>
                <td className={styles.mono}>{a.state.id.slice(0, 8)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}

function AgencyPlayground() {
  const [form, setForm] = useState<AgencyFormState>(DEFAULT_FORM);
  const [creation, setCreation] = useState<AgencyCreateResponse | null>(null);
  const [createError, setCreateError] = useState<string | null>(null);
  const [spawnError, setSpawnError] = useState<string | null>(null);
  const [spawnedSwarmId, setSpawnedSwarmId] = useState<string | null>(null);
  const [isCreating, setIsCreating] = useState(false);
  const [isSpawning, setIsSpawning] = useState(false);
  const { data: providerCatalog } = usePolled(() => providers.list(), 60000);
  const createdAgency = creation?.agency ?? null;

  useEffect(() => {
    if (!providerCatalog || form.provider) {
      return;
    }

    const preferred =
      providerCatalog.find((provider) => provider.configured) ?? providerCatalog[0];
    if (!preferred) {
      return;
    }

    setForm((current) =>
      current.provider ? current : { ...current, provider: preferred.id }
    );
  }, [providerCatalog, form.provider]);

  function updateField<Key extends keyof AgencyFormState>(
    key: Key,
    value: AgencyFormState[Key]
  ) {
    setForm((current) => ({ ...current, [key]: value }));
  }

  async function handleGenerate(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const selectedModel = form.model.trim() || defaultModelForProvider(form.provider);

    if (!form.provider) {
      setCreateError('choose a provider before creating an agency');
      return;
    }

    if (!selectedModel) {
      setCreateError('enter a model for the selected provider');
      return;
    }

    setIsCreating(true);
    setCreateError(null);
    setSpawnError(null);
    setSpawnedSwarmId(null);

    try {
      const created = await agencies.create({
        name: form.name,
        description: form.description,
        teamSize: form.teamSize,
        provider: form.provider,
        model: selectedModel,
        modelPool: form.modelPool
          .split(',')
          .map((item) => item.trim())
          .filter(Boolean),
        outputDir: form.outputDir.trim() || undefined,
        seedMemories: form.seedMemories,
        overwrite: form.overwrite,
      });
      setCreation(created);
    } catch (error) {
      setCreation(null);
      setCreateError(error instanceof Error ? error.message : String(error));
    } finally {
      setIsCreating(false);
    }
  }

  async function handleSpawn() {
    if (!createdAgency) {
      return;
    }

    setIsSpawning(true);
    setSpawnError(null);
    setSpawnedSwarmId(null);

    try {
      const swarm = await swarms.create(buildSwarmRequest(createdAgency, form.strategy));
      setSpawnedSwarmId(swarm.id);
    } catch (error) {
      setSpawnError(error instanceof Error ? error.message : String(error));
    } finally {
      setIsSpawning(false);
    }
  }

  return (
    <section className={styles.panel}>
      <div className={styles.panelHeader}>
        <div>
          <h2 className={styles.panelTitle}>agency playground</h2>
          <p className={styles.muted}>
            Create a CLI-style agency workspace through the daemon, review the generated team, then optionally spawn it as a live swarm.
          </p>
        </div>
      </div>

      <form className={styles.form} onSubmit={handleGenerate}>
        <div className={styles.formGrid}>
          <label className={styles.field}>
            <span className={styles.fieldLabel}>agency name</span>
            <input
              className={styles.input}
              value={form.name}
              onChange={(event) => updateField('name', event.target.value)}
              placeholder="Northstar Studio"
            />
          </label>

          <label className={styles.field}>
            <span className={styles.fieldLabel}>team size</span>
            <input
              className={styles.input}
              type="number"
              min={2}
              max={10}
              value={form.teamSize}
              onChange={(event) =>
                updateField('teamSize', Number(event.target.value) || DEFAULT_FORM.teamSize)
              }
            />
          </label>

          <label className={styles.field}>
            <span className={styles.fieldLabel}>provider</span>
            <select
              className={styles.input}
              value={form.provider}
              onChange={(event) => updateField('provider', event.target.value)}
            >
              <option value="">choose provider</option>
              {(providerCatalog ?? []).map((provider: ProviderResponse) => (
                <option key={provider.id} value={provider.id}>
                  {provider.label}
                  {provider.configured ? ' · configured' : ''}
                </option>
              ))}
            </select>
          </label>

          <label className={styles.field}>
            <span className={styles.fieldLabel}>strategy</span>
            <select
              className={styles.input}
              value={form.strategy}
              onChange={(event) =>
                updateField('strategy', event.target.value as SwarmStrategy)
              }
            >
              <option value="supervisor">supervisor</option>
              <option value="dynamic">dynamic</option>
              <option value="round-robin">round-robin</option>
            </select>
          </label>

          <label className={styles.field}>
            <span className={styles.fieldLabel}>model</span>
            <input
              className={styles.input}
              value={form.model}
              onChange={(event) => updateField('model', event.target.value)}
              placeholder={modelPlaceholder(form.provider)}
            />
          </label>

          <label className={styles.field}>
            <span className={styles.fieldLabel}>model pool</span>
            <input
              className={styles.input}
              value={form.modelPool}
              onChange={(event) => updateField('modelPool', event.target.value)}
              placeholder="Optional comma-separated models for role diversity"
            />
          </label>

          <label className={styles.field}>
            <span className={styles.fieldLabel}>output directory</span>
            <input
              className={styles.input}
              value={form.outputDir}
              onChange={(event) => updateField('outputDir', event.target.value)}
              placeholder="Optional; defaults to the agency slug"
            />
          </label>

          <label className={`${styles.field} ${styles.fieldWide}`}>
            <span className={styles.fieldLabel}>description</span>
            <textarea
              className={`${styles.input} ${styles.textarea}`}
              value={form.description}
              onChange={(event) => updateField('description', event.target.value)}
              placeholder="What kind of agency are you building?"
            />
          </label>

          <div className={`${styles.field} ${styles.fieldWide}`}>
            <span className={styles.fieldLabel}>creation options</span>
            <div className={styles.checkboxRow}>
              <label className={styles.checkboxLabel}>
                <input
                  type="checkbox"
                  checked={form.seedMemories}
                  onChange={(event) => updateField('seedMemories', event.target.checked)}
                />
                <span>write seed memories like `animaos create --seed`</span>
              </label>
              <label className={styles.checkboxLabel}>
                <input
                  type="checkbox"
                  checked={form.overwrite}
                  onChange={(event) => updateField('overwrite', event.target.checked)}
                />
                <span>overwrite the target directory if it already exists</span>
              </label>
            </div>
          </div>
        </div>

        <div className={styles.buttonRow}>
          <button className={styles.actionButton} type="submit" disabled={isCreating}>
            {isCreating ? 'Creating agency workspace…' : 'Create agency workspace'}
          </button>
          <span className={styles.muted}>
            The daemon generates the team, writes the workspace files, and keeps provider keys server-side.
          </span>
        </div>
      </form>

      {createError && <p className={styles.err}>{createError}</p>}
      {spawnError && <p className={styles.err}>{spawnError}</p>}
      {creation && (
        <p className={styles.ok}>
          created workspace {creation.outputDir}
          {creation.seedMemoryCount > 0
            ? ` · ${creation.seedMemoryCount} seed memories across ${creation.seededAgents} agents`
            : ''}
        </p>
      )}
      {spawnedSwarmId && (
        <p className={styles.ok}>spawned live swarm {spawnedSwarmId.slice(0, 8)}</p>
      )}

      {createdAgency && creation && (
        <div className={styles.preview}>
          <div className={styles.previewHeader}>
            <div>
              <h3 className={styles.previewTitle}>{createdAgency.name}</h3>
              <p className={styles.muted}>{createdAgency.description}</p>
            </div>
            <div className={styles.buttonRow}>
              <button
                className={`${styles.actionButton} ${styles.actionButtonGhost}`}
                type="button"
                onClick={handleSpawn}
                disabled={isSpawning}
              >
                {isSpawning ? 'Spawning swarm…' : 'Spawn as live swarm'}
              </button>
            </div>
          </div>

          <div className={styles.previewMeta}>
            <span className={styles.metaBadge}>provider {createdAgency.provider}</span>
            <span className={styles.metaBadge}>model {createdAgency.model}</span>
            <span className={styles.metaBadge}>team {createdAgency.teamSize}</span>
            <span className={styles.metaBadge}>strategy {form.strategy}</span>
            <span className={styles.metaBadge}>workspace {creation.outputDir}</span>
          </div>

          {createdAgency.mission && (
            <div className={styles.copyBlock}>
              <h4 className={styles.copyTitle}>mission</h4>
              <p className={styles.copyText}>{createdAgency.mission}</p>
            </div>
          )}

          {createdAgency.values && createdAgency.values.length > 0 && (
            <div className={styles.copyBlock}>
              <h4 className={styles.copyTitle}>values</h4>
              <div className={styles.tagRow}>
                {createdAgency.values.map((value) => (
                  <span key={value} className={styles.tag}>
                    {value}
                  </span>
                ))}
              </div>
            </div>
          )}

          <div className={styles.copyBlock}>
            <h4 className={styles.copyTitle}>workspace files</h4>
            <ul className={styles.fileList}>
              {creation.files.map((file) => (
                <li key={file} className={styles.fileItem}>
                  {file}
                </li>
              ))}
            </ul>
          </div>

          <div className={styles.cards}>
            {createdAgency.agents.map((agent) => (
              <article key={`${agent.role}-${agent.name}`} className={styles.agentCard}>
                <div className={styles.agentHeader}>
                  <div>
                    <h4 className={styles.agentName}>{agent.name}</h4>
                    {agent.position && (
                      <p className={styles.agentPosition}>{agent.position}</p>
                    )}
                  </div>
                  <span
                    className={styles.roleBadge}
                    data-role={agent.role}
                  >
                    {agent.role}
                  </span>
                </div>

                {agent.bio && <p className={styles.copyText}>{agent.bio}</p>}

                <dl className={styles.agentFacts}>
                  {agent.model && (
                    <div>
                      <dt>model</dt>
                      <dd>{agent.model}</dd>
                    </div>
                  )}
                  {agent.topics && agent.topics.length > 0 && (
                    <div>
                      <dt>topics</dt>
                      <dd>{agent.topics.join(', ')}</dd>
                    </div>
                  )}
                  {agent.tools && agent.tools.length > 0 && (
                    <div>
                      <dt>tools</dt>
                      <dd>{agent.tools.join(', ')}</dd>
                    </div>
                  )}
                  {agent.collaboratesWith && agent.collaboratesWith.length > 0 && (
                    <div>
                      <dt>pairs with</dt>
                      <dd>{agent.collaboratesWith.join(', ')}</dd>
                    </div>
                  )}
                </dl>

                {agent.system && (
                  <div className={styles.systemBlock}>
                    <h5 className={styles.copyTitle}>system</h5>
                    <p className={styles.systemText}>{agent.system}</p>
                  </div>
                )}
              </article>
            ))}
          </div>
        </div>
      )}
    </section>
  );
}

function SwarmsPanel() {
  const { data, error, loading } = usePolled(() => swarms.list());
  return (
    <section className={styles.panel}>
      <h2 className={styles.panelTitle}>swarms</h2>
      {loading && <p className={styles.muted}>loading…</p>}
      {error && <p className={styles.err}>{error}</p>}
      {data && data.length === 0 && <p className={styles.muted}>no swarms</p>}
      {data && data.length > 0 && (
        <table className={styles.table}>
          <thead>
            <tr>
              <th>id</th>
              <th>status</th>
              <th>agents</th>
              <th>tokens</th>
            </tr>
          </thead>
          <tbody>
            {data.map((s: SwarmState) => (
              <tr key={s.id}>
                <td className={styles.mono}>{s.id.slice(0, 8)}</td>
                <td>
                  <StatusDot status={s.status} />
                  {s.status}
                </td>
                <td>{s.agentIds.length}</td>
                <td>{s.tokenUsage.totalTokens.toLocaleString()}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}

export function App() {
  const { data: h, error: hErr } = usePolled(() => health.get(), 8000);

  return (
    <div className={styles.shell}>
      <header className={styles.header}>
        <span className={styles.kicker}>animaOS</span>
        <h1 className={styles.title}>Control Grid</h1>
        <div className={styles.headerRight}>
          <HealthBadge h={h} error={hErr} />
          <button
            type="button"
            className={`${styles.actionButton} ${styles.actionButtonGhost}`}
            onClick={() => location.reload()}
          >
            Reload
          </button>
        </div>
      </header>

      <main className={styles.main}>
        <AgencyPlayground />
        <AgentsPanel />
        <SwarmsPanel />
      </main>
    </div>
  );
}

export default App;

function buildSwarmRequest(
  draft: AgencyGenerateResponse,
  strategy: SwarmStrategy
): SwarmCreateRequest {
  const managerIndex = draft.agents.findIndex(
    (agent) => agent.role === 'orchestrator'
  );
  const safeManagerIndex = managerIndex >= 0 ? managerIndex : 0;
  const manager = draft.agents[safeManagerIndex];
  const workers = draft.agents.filter((_, index) => index !== safeManagerIndex);

  if (!manager || workers.length === 0) {
    throw new Error('generated agency must include one orchestrator and at least one worker');
  }

  return {
    strategy,
    manager: toAgentConfig(manager, draft),
    workers: workers.map((agent) => toAgentConfig(agent, draft)),
    maxConcurrentAgents: draft.teamSize,
    maxParallelDelegations:
      strategy === 'supervisor' ? Math.max(1, workers.length) : undefined,
    maxTurns: strategy === 'round-robin' ? 6 : undefined,
  };
}

function toAgentConfig(
  agent: AgentDefinitionResponse,
  draft: AgencyGenerateResponse
): AgentConfig {
  return {
    name: agent.name,
    model: agent.model ?? draft.model,
    provider: draft.provider,
    bio: agent.bio,
    lore: agent.lore,
    knowledge: agent.knowledge,
    topics: agent.topics,
    adjectives: agent.adjectives,
    style: agent.style,
    system: agent.system,
    tools: agent.tools,
  };
}

function modelPlaceholder(provider: string): string {
  return defaultModelForProvider(provider) ?? 'Enter a model id';
}

function defaultModelForProvider(provider: string): string | undefined {
  switch (provider) {
    case 'anthropic':
      return 'claude-sonnet-4-6';
    case 'google':
      return 'gemini-2.5-pro';
    case 'ollama':
      return 'qwen3:latest';
    case 'openai':
      return 'gpt-4o-mini';
    default:
      return undefined;
  }
}
