import { useState } from 'react';
import {
  swarms,
  type AgentConfig,
  type Provider,
  type SwarmCreateRequest,
} from '../lib/api';
import { Field, FormError, FormHeader, Row } from './CreateAgent';
import { ProviderSelect } from './ProviderSelect';

interface Props {
  onCreated: (id: string) => void;
  onCancel: () => void;
  providers: Provider[] | null;
}

interface AgentDraft {
  name: string;
  model: string;
  provider: string;
  system: string;
}

const blank = (name = ''): AgentDraft => ({
  name,
  model: 'claude-sonnet-4-6',
  provider: '',
  system: '',
});

export function CreateSwarm({ onCreated, onCancel, providers }: Props) {
  const [strategy, setStrategy] =
    useState<SwarmCreateRequest['strategy']>('supervisor');
  const [manager, setManager] = useState<AgentDraft>(blank('manager'));
  const [workers, setWorkers] = useState<AgentDraft[]>([blank('worker-1')]);
  const [maxTurns, setMaxTurns] = useState('');
  const [tokenBudget, setTokenBudget] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  function patchManager(patch: Partial<AgentDraft>) {
    setManager((m) => ({ ...m, ...patch }));
  }
  function patchWorker(i: number, patch: Partial<AgentDraft>) {
    setWorkers((ws) => ws.map((w, j) => (i === j ? { ...w, ...patch } : w)));
  }
  function addWorker() {
    setWorkers((ws) => [...ws, blank(`worker-${ws.length + 1}`)]);
  }
  function removeWorker(i: number) {
    setWorkers((ws) => ws.filter((_, j) => j !== i));
  }

  function toConfig(d: AgentDraft): AgentConfig {
    return {
      name: d.name.trim(),
      model: d.model.trim(),
      provider: d.provider.trim() || undefined,
      system: d.system.trim() || undefined,
    };
  }

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setErr(null);
    if (workers.length === 0) {
      setErr('Add at least one worker.');
      return;
    }
    setSubmitting(true);
    try {
      const body: SwarmCreateRequest = {
        strategy,
        manager: toConfig(manager),
        workers: workers.map(toConfig),
      };
      if (maxTurns.trim()) body.maxTurns = parseInt(maxTurns, 10);
      if (tokenBudget.trim()) body.tokenBudget = parseInt(tokenBudget, 10);
      const created = await swarms.create(body);
      onCreated(created.id);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <form
      onSubmit={submit}
      className="max-w-2xl mx-auto px-6 py-8 flex flex-col gap-5"
    >
      <FormHeader
        title="New swarm"
        subtitle="A manager and one or more workers running under a coordination strategy."
        onCancel={onCancel}
      />

      <Field label="Strategy" required>
        <select
          value={strategy}
          onChange={(e) =>
            setStrategy(e.target.value as SwarmCreateRequest['strategy'])
          }
        >
          <option value="supervisor">Supervisor</option>
          <option value="dynamic">Dynamic</option>
          <option value="round-robin">Round-robin</option>
        </select>
      </Field>

      <Card title="Manager">
        <AgentFields
          draft={manager}
          onChange={patchManager}
          providers={providers}
        />
      </Card>

      <Card
        title={`Workers (${workers.length})`}
        action={
          <button
            type="button"
            onClick={addWorker}
            className="text-sm px-2.5 py-1 rounded-md text-[var(--accent)] hover:bg-[var(--accent-soft)] transition-colors"
          >
            + Add worker
          </button>
        }
      >
        <div className="flex flex-col gap-3">
          {workers.map((w, i) => (
            <div
              key={i}
              className="rounded-md border border-[var(--border)] bg-[var(--surface)] p-4 flex flex-col gap-3"
            >
              <div className="flex items-center justify-between">
                <span className="text-sm font-medium text-[var(--text-2)]">
                  Worker {i + 1}
                </span>
                <button
                  type="button"
                  onClick={() => removeWorker(i)}
                  className="text-sm text-[var(--muted)] hover:text-[var(--err)] transition-colors"
                >
                  Remove
                </button>
              </div>
              <AgentFields
                draft={w}
                onChange={(patch) => patchWorker(i, patch)}
                providers={providers}
              />
            </div>
          ))}
        </div>
      </Card>

      <Row>
        <Field label="Max turns">
          <input
            type="number"
            min="1"
            value={maxTurns}
            onChange={(e) => setMaxTurns(e.target.value)}
            placeholder="10"
          />
        </Field>
        <Field label="Token budget">
          <input
            type="number"
            min="1"
            value={tokenBudget}
            onChange={(e) => setTokenBudget(e.target.value)}
            placeholder="50000"
          />
        </Field>
      </Row>

      {err && <FormError message={err} />}

      <div className="flex gap-2 mt-2">
        <button
          type="submit"
          disabled={submitting || !manager.name.trim() || workers.length === 0}
          className="px-4 py-2 text-sm font-medium rounded-md bg-[var(--accent)] text-[var(--accent-fg)] hover:bg-[var(--accent-hover)] disabled:bg-[var(--surface-3)] disabled:text-[var(--muted-2)] disabled:cursor-not-allowed transition-colors"
        >
          {submitting ? 'Creating…' : 'Create swarm'}
        </button>
        <button
          type="button"
          onClick={onCancel}
          className="px-4 py-2 text-sm rounded-md text-[var(--muted)] hover:text-[var(--text)] hover:bg-[var(--hover)] transition-colors"
        >
          Cancel
        </button>
      </div>
    </form>
  );
}

function Card({
  title,
  action,
  children,
}: {
  title: string;
  action?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <section className="rounded-lg border border-[var(--border)] bg-[var(--surface-2)]/50 p-5 flex flex-col gap-4">
      <div className="flex items-center justify-between">
        <h3 className="text-sm font-semibold text-[var(--text)] m-0">{title}</h3>
        {action}
      </div>
      {children}
    </section>
  );
}

function AgentFields({
  draft,
  onChange,
  providers,
}: {
  draft: AgentDraft;
  onChange: (patch: Partial<AgentDraft>) => void;
  providers: Provider[] | null;
}) {
  return (
    <>
      <Row>
        <Field label="Name" required>
          <input
            value={draft.name}
            onChange={(e) => onChange({ name: e.target.value })}
            required
          />
        </Field>
        <Field label="Model" required>
          <input
            value={draft.model}
            onChange={(e) => onChange({ model: e.target.value })}
            required
          />
        </Field>
      </Row>
      <Row>
        <Field label="Provider">
          <ProviderSelect
            providers={providers}
            value={draft.provider}
            onChange={(v) => onChange({ provider: v })}
          />
        </Field>
        <Field label="System">
          <input
            value={draft.system}
            onChange={(e) => onChange({ system: e.target.value })}
          />
        </Field>
      </Row>
    </>
  );
}
