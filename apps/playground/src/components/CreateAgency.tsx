import { useState } from 'react';
import {
  agencies,
  swarms,
  type AgencyCreateResponse,
  type AgencyGenerateResponse,
  type AgentConfig,
  type AgentDefinitionResponse,
  type Provider,
  type SwarmCreateRequest,
} from '../lib/api';
import { Field, FormError, FormHeader, Row } from './CreateAgent';
import { ProviderSelect } from './ProviderSelect';

interface Props {
  onCancel: () => void;
  onSwarmCreated: (id: string) => void;
  providers: Provider[] | null;
}

type SwarmStrategy = SwarmCreateRequest['strategy'];

export function CreateAgency({ onCancel, onSwarmCreated, providers }: Props) {
  const [name, setName] = useState('Northstar Studio');
  const [description, setDescription] = useState(
    'A strategic creative agency that turns messy product and growth ideas into clear campaigns, launch plans, and messaging systems.'
  );
  const [teamSize, setTeamSize] = useState('4');
  const [provider, setProvider] = useState('');
  const [model, setModel] = useState('claude-sonnet-4-6');
  const [modelPool, setModelPool] = useState('');
  const [outputDir, setOutputDir] = useState('');
  const [strategy, setStrategy] = useState<SwarmStrategy>('supervisor');
  const [seedMemories, setSeedMemories] = useState(false);
  const [overwrite, setOverwrite] = useState(false);
  const [creation, setCreation] = useState<AgencyCreateResponse | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [spawning, setSpawning] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    setErr(null);
    setSubmitting(true);
    try {
      const created = await agencies.create({
        name: name.trim(),
        description: description.trim(),
        teamSize: Number(teamSize) || 4,
        provider: provider.trim() || undefined,
        model: model.trim(),
        modelPool: modelPool
          .split(',')
          .map((item) => item.trim())
          .filter(Boolean),
        outputDir: outputDir.trim() || undefined,
        seedMemories,
        overwrite,
      });
      setCreation(created);
    } catch (error) {
      setCreation(null);
      setErr(error instanceof Error ? error.message : String(error));
    } finally {
      setSubmitting(false);
    }
  }

  async function handleSpawn() {
    if (!creation) {
      return;
    }

    setErr(null);
    setSpawning(true);
    try {
      const created = await swarms.create(
        buildSwarmRequest(creation.agency, strategy)
      );
      onSwarmCreated(created.id);
    } catch (error) {
      setErr(error instanceof Error ? error.message : String(error));
    } finally {
      setSpawning(false);
    }
  }

  return (
    <div className="h-full overflow-y-auto">
      <form
        onSubmit={submit}
        className="max-w-4xl mx-auto px-6 py-8 flex flex-col gap-5"
      >
        <FormHeader
          title="Create agency from prompt"
          subtitle="Similar to the CLI: describe the agency, create the workspace files, then optionally spawn the returned team as a live swarm."
          onCancel={onCancel}
        />

        <Row>
          <Field label="Agency name" required>
            <input
              value={name}
              onChange={(event) => setName(event.target.value)}
              required
              autoFocus
            />
          </Field>
          <Field label="Team size" required>
            <input
              type="number"
              min="2"
              max="10"
              value={teamSize}
              onChange={(event) => setTeamSize(event.target.value)}
              required
            />
          </Field>
        </Row>

        <Field label="Agency prompt" required hint="This is the CLI-style description the daemon uses to generate the team.">
          <textarea
            rows={5}
            value={description}
            onChange={(event) => setDescription(event.target.value)}
            required
          />
        </Field>

        <Row>
          <Field label="Provider">
            <ProviderSelect
              providers={providers}
              value={provider}
              onChange={setProvider}
            />
          </Field>
          <Field label="Model" required>
            <input
              value={model}
              onChange={(event) => setModel(event.target.value)}
              required
            />
          </Field>
        </Row>

        <Row>
          <Field label="Model pool" hint="Optional comma-separated pool for role diversity.">
            <input
              value={modelPool}
              onChange={(event) => setModelPool(event.target.value)}
              placeholder="gemma4:31b,llama3.1:70b"
            />
          </Field>
          <Field label="Output directory" hint="Optional workspace folder name.">
            <input
              value={outputDir}
              onChange={(event) => setOutputDir(event.target.value)}
              placeholder="northstar-studio"
            />
          </Field>
        </Row>

        <Row>
          <Field label="Spawn strategy">
            <select
              value={strategy}
              onChange={(event) =>
                setStrategy(event.target.value as SwarmStrategy)
              }
            >
              <option value="supervisor">Supervisor</option>
              <option value="dynamic">Dynamic</option>
              <option value="round-robin">Round-robin</option>
            </select>
          </Field>
          <div className="flex flex-col gap-2">
            <span className="text-sm text-[var(--text-2)] font-medium">Options</span>
            <label className="flex items-center gap-2 text-sm text-[var(--muted)]">
              <input
                type="checkbox"
                checked={seedMemories}
                onChange={(event) => setSeedMemories(event.target.checked)}
                className="w-auto"
              />
              Generate seed memories like `animaos create --seed`
            </label>
            <label className="flex items-center gap-2 text-sm text-[var(--muted)]">
              <input
                type="checkbox"
                checked={overwrite}
                onChange={(event) => setOverwrite(event.target.checked)}
                className="w-auto"
              />
              Overwrite the target directory if it exists
            </label>
          </div>
        </Row>

        {err && <FormError message={err} />}

        <div className="flex gap-2 mt-2">
          <button
            type="submit"
            disabled={submitting || !name.trim() || !description.trim() || !model.trim()}
            className="px-4 py-2 text-sm font-medium rounded-md bg-[var(--accent)] text-[var(--accent-fg)] hover:bg-[var(--accent-hover)] disabled:bg-[var(--surface-3)] disabled:text-[var(--muted-2)] disabled:cursor-not-allowed transition-colors"
          >
            {submitting ? 'Creating…' : 'Create agency workspace'}
          </button>
          <button
            type="button"
            onClick={onCancel}
            className="px-4 py-2 text-sm rounded-md text-[var(--muted)] hover:text-[var(--text)] hover:bg-[var(--hover)] transition-colors"
          >
            Cancel
          </button>
        </div>

        {creation && (
          <section className="rounded-lg border border-[var(--border)] bg-[var(--surface-2)]/50 p-5 flex flex-col gap-4 animate-[fade-in_120ms_ease]">
            <div className="flex flex-wrap items-start justify-between gap-3">
              <div className="flex flex-col gap-1">
                <h3 className="text-base font-semibold text-[var(--text)] m-0">
                  {creation.agency.name}
                </h3>
                <p className="text-sm text-[var(--muted)] m-0">
                  workspace: {creation.outputDir}
                </p>
              </div>
              <button
                type="button"
                onClick={handleSpawn}
                disabled={spawning}
                className="px-4 py-2 text-sm rounded-md border border-[var(--border-strong)] text-[var(--text)] hover:bg-[var(--hover)] disabled:bg-[var(--surface-3)] disabled:text-[var(--muted-2)] disabled:cursor-not-allowed transition-colors"
              >
                {spawning ? 'Spawning…' : 'Spawn as live swarm'}
              </button>
            </div>

            <div className="flex flex-wrap gap-2 text-xs text-[var(--muted)]">
              <span className="px-2.5 py-1 rounded-full border border-[var(--border)] bg-[var(--surface)]">
                provider {creation.agency.provider}
              </span>
              <span className="px-2.5 py-1 rounded-full border border-[var(--border)] bg-[var(--surface)]">
                model {creation.agency.model}
              </span>
              <span className="px-2.5 py-1 rounded-full border border-[var(--border)] bg-[var(--surface)]">
                team {creation.agency.teamSize}
              </span>
              {creation.seedMemoryCount > 0 && (
                <span className="px-2.5 py-1 rounded-full border border-[var(--border)] bg-[var(--surface)]">
                  seeds {creation.seedMemoryCount} across {creation.seededAgents} agents
                </span>
              )}
            </div>

            {creation.agency.mission && (
              <div className="flex flex-col gap-1">
                <span className="text-xs uppercase tracking-[0.18em] text-[var(--muted-2)]">Mission</span>
                <p className="text-sm text-[var(--text-2)] m-0">{creation.agency.mission}</p>
              </div>
            )}

            {creation.agency.values && creation.agency.values.length > 0 && (
              <div className="flex flex-col gap-2">
                <span className="text-xs uppercase tracking-[0.18em] text-[var(--muted-2)]">Values</span>
                <div className="flex flex-wrap gap-2">
                  {creation.agency.values.map((value) => (
                    <span
                      key={value}
                      className="px-2.5 py-1 rounded-full border border-[var(--border)] bg-[var(--surface)] text-xs text-[var(--text-2)]"
                    >
                      {value}
                    </span>
                  ))}
                </div>
              </div>
            )}

            <div className="grid lg:grid-cols-[minmax(0,1fr)_280px] gap-4">
              <div className="flex flex-col gap-3">
                <span className="text-xs uppercase tracking-[0.18em] text-[var(--muted-2)]">Team</span>
                <div className="grid md:grid-cols-2 gap-3">
                  {creation.agency.agents.map((agent) => (
                    <article
                      key={`${agent.role}-${agent.name}`}
                      className="rounded-md border border-[var(--border)] bg-[var(--surface)] p-4 flex flex-col gap-2"
                    >
                      <div className="flex items-start justify-between gap-2">
                        <div>
                          <h4 className="text-sm font-semibold text-[var(--text)] m-0">{agent.name}</h4>
                          {agent.position && (
                            <p className="text-xs text-[var(--muted)] m-0 mt-0.5">{agent.position}</p>
                          )}
                        </div>
                        <span className="px-2 py-0.5 rounded-full border border-[var(--border)] text-[10px] uppercase tracking-[0.14em] text-[var(--text-2)]">
                          {agent.role}
                        </span>
                      </div>
                      {agent.bio && (
                        <p className="text-sm text-[var(--text-2)] m-0">{agent.bio}</p>
                      )}
                      {agent.tools && agent.tools.length > 0 && (
                        <p className="text-xs text-[var(--muted)] m-0">
                          tools: {agent.tools.join(', ')}
                        </p>
                      )}
                      {agent.collaboratesWith && agent.collaboratesWith.length > 0 && (
                        <p className="text-xs text-[var(--muted)] m-0">
                          collaborates with: {agent.collaboratesWith.join(', ')}
                        </p>
                      )}
                    </article>
                  ))}
                </div>
              </div>

              <div className="flex flex-col gap-3 min-w-0">
                <span className="text-xs uppercase tracking-[0.18em] text-[var(--muted-2)]">Created files</span>
                <div className="rounded-md border border-[var(--border)] bg-[var(--surface)] p-3 overflow-auto max-h-[22rem]">
                  <ul className="m-0 pl-4 flex flex-col gap-1">
                    {creation.files.map((file) => (
                      <li key={file} className="text-xs text-[var(--text-2)] break-all">
                        {file}
                      </li>
                    ))}
                  </ul>
                </div>
              </div>
            </div>
          </section>
        )}
      </form>
    </div>
  );
}

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