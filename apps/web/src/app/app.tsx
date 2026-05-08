import { useEffect, useMemo, useRef, useState } from 'react';
import {
  agents,
  health,
  memories,
  providers as providerApi,
  swarms,
  type AgentConfig,
  type AgentSnapshot,
  type HealthResponse,
  type Memory,
  type MemorySearchResult,
  type ProviderResponse,
  type SwarmCreateRequest,
  type SwarmState,
  type SwarmStreamEvent,
  type WorkerConfig,
} from '../lib/api';

// ── Shared styles ─────────────────────────────────────────────────────────────
const INPUT =
  'w-full rounded-xl border border-slate-200 bg-slate-50 px-4 py-2.5 text-sm text-slate-900 outline-none transition focus:border-orange-300 focus:ring-2 focus:ring-orange-200';

// ── Field label wrapper ───────────────────────────────────────────────────────
function Field({
  label,
  hint,
  required,
  children,
}: {
  label: string;
  hint?: string;
  required?: boolean;
  children: React.ReactNode;
}) {
  return (
    <div className="space-y-1">
      <label className="flex flex-wrap items-center gap-1 text-xs font-semibold uppercase tracking-[0.2em] text-slate-500">
        {label}
        {required && <span className="text-rose-500">*</span>}
        {hint && (
          <span className="ml-0.5 font-normal normal-case tracking-normal text-slate-400">
            — {hint}
          </span>
        )}
      </label>
      {children}
    </div>
  );
}

// ── Modal backdrop ────────────────────────────────────────────────────────────
function Modal({ onClose, children }: { onClose: () => void; children: React.ReactNode }) {
  useEffect(() => {
    const handle = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', handle);
    return () => window.removeEventListener('keydown', handle);
  }, [onClose]);

  return (
    <div
      role="dialog"
      aria-modal="true"
      className="fixed inset-0 z-50 flex items-center justify-center bg-slate-950/60 p-4 backdrop-blur-sm"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div className="relative max-h-[92vh] w-full max-w-lg overflow-y-auto rounded-[2rem] border border-white/60 bg-white shadow-[0_40px_80px_-20px_rgba(15,23,42,0.5)]">
        {children}
      </div>
    </div>
  );
}

function ModalClose({ onClose }: { onClose: () => void }) {
  return (
    <button
      type="button"
      onClick={onClose}
      aria-label="Close"
      className="mt-0.5 rounded-full p-2 text-slate-400 transition hover:bg-slate-100 hover:text-slate-700"
    >
      <svg className="h-4 w-4" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="2">
        <path d="M2 2l12 12M14 2L2 14" strokeLinecap="round" />
      </svg>
    </button>
  );
}

function ErrorBox({ children }: { children: React.ReactNode }) {
  return (
    <div className="rounded-xl border border-rose-200 bg-rose-50 px-4 py-3 text-sm text-rose-700">
      {children}
    </div>
  );
}

function ModalActions({
  onClose,
  closeable,
  children,
}: {
  onClose: () => void;
  closeable: boolean;
  children: React.ReactNode;
}) {
  return (
    <div className="flex justify-end gap-3 pt-2">
      <button
        type="button"
        onClick={onClose}
        disabled={!closeable}
        className="rounded-xl border border-slate-200 px-4 py-2.5 text-sm font-medium text-slate-600 transition hover:bg-slate-50 disabled:opacity-40"
      >
        Cancel
      </button>
      {children}
    </div>
  );
}

type ButtonAccent = 'orange' | 'emerald' | 'violet' | 'sky';
const ACCENT_BG: Record<ButtonAccent, string> = {
  orange: 'bg-orange-600 hover:bg-orange-500',
  emerald: 'bg-emerald-600 hover:bg-emerald-500',
  violet: 'bg-violet-600 hover:bg-violet-500',
  sky: 'bg-sky-600 hover:bg-sky-500',
};

function SubmitButton({
  disabled,
  loading,
  accent,
  children,
}: {
  disabled: boolean;
  loading: boolean;
  accent: ButtonAccent;
  children: React.ReactNode;
}) {
  return (
    <button
      type="submit"
      disabled={disabled}
      className={`rounded-xl px-5 py-2.5 text-sm font-semibold text-white transition ${ACCENT_BG[accent]} disabled:cursor-not-allowed disabled:opacity-50`}
    >
      {loading ? 'Working…' : children}
    </button>
  );
}

// ── New Agent Modal ───────────────────────────────────────────────────────────
function NewAgentModal({
  configuredProviders,
  onClose,
  onCreated,
}: {
  configuredProviders: ProviderResponse[];
  onClose: () => void;
  onCreated: (agent: AgentSnapshot) => void;
}) {
  const [name, setName] = useState('');
  const [model, setModel] = useState('');
  const [provider, setProvider] = useState(configuredProviders[0]?.id ?? '');
  const [bio, setBio] = useState('');
  const [system, setSystem] = useState('');
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!name.trim()) return;
    setCreating(true);
    setError(null);
    try {
      const config: AgentConfig = {
        name: name.trim(),
        model: model.trim() || 'gpt-4o-mini',
        provider: provider || configuredProviders[0]?.id || 'openai',
        ...(bio.trim() ? { bio: bio.trim() } : {}),
        ...(system.trim() ? { system: system.trim() } : {}),
      };
      onCreated(await agents.create(config));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setCreating(false);
    }
  }

  return (
    <Modal onClose={onClose}>
      <div className="px-6 py-6 sm:px-8">
        <div className="mb-6 flex items-start justify-between gap-4">
          <div>
            <p className="mb-1 inline-flex rounded-full bg-orange-100 px-2.5 py-0.5 text-xs font-semibold uppercase tracking-[0.2em] text-orange-700">
              New agent
            </p>
            <h2 className="text-xl font-semibold text-slate-950">Create an agent</h2>
            <p className="mt-1 text-sm text-slate-500">Registers a new agent on the running daemon.</p>
          </div>
          <ModalClose onClose={onClose} />
        </div>

        <form onSubmit={handleSubmit} className="space-y-4">
          <Field label="Name" required>
            <input
              autoFocus
              className={INPUT}
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="e.g. Research assistant"
              disabled={creating}
            />
          </Field>

          <div className="grid gap-4 sm:grid-cols-2">
            <Field label="Provider">
              {configuredProviders.length ? (
                <select
                  className={INPUT}
                  value={provider}
                  onChange={(e) => setProvider(e.target.value)}
                  disabled={creating}
                >
                  {configuredProviders.map((p) => (
                    <option key={p.id} value={p.id}>
                      {p.label}
                    </option>
                  ))}
                </select>
              ) : (
                <input
                  className={INPUT}
                  value={provider}
                  onChange={(e) => setProvider(e.target.value)}
                  placeholder="openai"
                  disabled={creating}
                />
              )}
            </Field>
            <Field label="Model">
              <input
                className={INPUT}
                value={model}
                onChange={(e) => setModel(e.target.value)}
                placeholder="gpt-4o-mini"
                disabled={creating}
              />
            </Field>
          </div>

          <Field label="Bio" hint="Optional character backstory">
            <textarea
              className={`${INPUT} min-h-[5rem] resize-none`}
              value={bio}
              onChange={(e) => setBio(e.target.value)}
              placeholder="This agent specialises in…"
              disabled={creating}
            />
          </Field>

          <Field label="System prompt" hint="Optional, overrides the default">
            <textarea
              className={`${INPUT} min-h-[5rem] resize-none`}
              value={system}
              onChange={(e) => setSystem(e.target.value)}
              placeholder="You are a helpful assistant that…"
              disabled={creating}
            />
          </Field>

          {error && <ErrorBox>{error}</ErrorBox>}

          <ModalActions onClose={onClose} closeable={!creating}>
            <SubmitButton disabled={creating || !name.trim()} loading={creating} accent="orange">
              Create agent
            </SubmitButton>
          </ModalActions>
        </form>
      </div>
    </Modal>
  );
}

// ── New Swarm Modal ───────────────────────────────────────────────────────────
const STRATEGY_INFO = {
  supervisor: {
    label: 'Supervisor',
    desc: 'Manager delegates subtasks to workers and aggregates results.',
  },
  dynamic: {
    label: 'Dynamic',
    desc: 'Workers self-organise based on incoming task requirements.',
  },
  'round-robin': {
    label: 'Round-robin',
    desc: 'Tasks are distributed evenly across all workers in rotation.',
  },
} as const;

type Strategy = keyof typeof STRATEGY_INFO;

function NewSwarmModal({
  configuredProviders,
  onClose,
  onCreated,
}: {
  configuredProviders: ProviderResponse[];
  onClose: () => void;
  onCreated: (swarm: SwarmState) => void;
}) {
  const [strategy, setStrategy] = useState<Strategy>('supervisor');
  const [mgrName, setMgrName] = useState('');
  const [mgrModel, setMgrModel] = useState('');
  const [workers, setWorkers] = useState([{ name: '', model: '' }]);
  const [maxTurns, setMaxTurns] = useState('');
  const [tokenBudget, setTokenBudget] = useState('');
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const updateWorker = (i: number, key: 'name' | 'model', val: string) =>
    setWorkers((prev) => prev.map((w, idx) => (idx === i ? { ...w, [key]: val } : w)));

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!mgrName.trim()) return;
    setCreating(true);
    setError(null);
    try {
      const fp = configuredProviders[0]?.id ?? 'openai';
      const filledWorkers: WorkerConfig[] = workers
        .filter((w) => w.name.trim())
        .map((w) => ({
          name: w.name.trim(),
          model: w.model.trim() || 'gpt-4o-mini',
          provider: fp,
        }));
      const req: SwarmCreateRequest = {
        strategy,
        manager: { name: mgrName.trim(), model: mgrModel.trim() || 'gpt-4o-mini', provider: fp },
        workers: filledWorkers.length
          ? filledWorkers
          : [{ name: `${mgrName.trim()}-worker`, model: 'gpt-4o-mini', provider: fp }],
        ...(maxTurns ? { maxTurns: Number(maxTurns) } : {}),
        ...(tokenBudget ? { tokenBudget: Number(tokenBudget) } : {}),
      };
      onCreated(await swarms.create(req));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setCreating(false);
    }
  }

  return (
    <Modal onClose={onClose}>
      <div className="px-6 py-6 sm:px-8">
        <div className="mb-6 flex items-start justify-between gap-4">
          <div>
            <p className="mb-1 inline-flex rounded-full bg-emerald-100 px-2.5 py-0.5 text-xs font-semibold uppercase tracking-[0.2em] text-emerald-700">
              New swarm
            </p>
            <h2 className="text-xl font-semibold text-slate-950">Create a swarm</h2>
            <p className="mt-1 text-sm text-slate-500">Spin up a multi-agent coordination swarm.</p>
          </div>
          <ModalClose onClose={onClose} />
        </div>

        <form onSubmit={handleSubmit} className="space-y-5">
          {/* Strategy selector */}
          <div>
            <p className="mb-2 text-xs font-semibold uppercase tracking-[0.2em] text-slate-500">Strategy</p>
            <div className="grid grid-cols-3 gap-2">
              {(Object.keys(STRATEGY_INFO) as Strategy[]).map((key) => (
                <button
                  key={key}
                  type="button"
                  onClick={() => setStrategy(key)}
                  className={[
                    'rounded-xl border px-3 py-2.5 text-left text-xs font-medium transition',
                    strategy === key
                      ? 'border-emerald-400 bg-emerald-50 text-emerald-700'
                      : 'border-slate-200 text-slate-600 hover:border-slate-300 hover:bg-slate-50',
                  ].join(' ')}
                >
                  {STRATEGY_INFO[key].label}
                </button>
              ))}
            </div>
            <p className="mt-2 text-xs text-slate-500">{STRATEGY_INFO[strategy].desc}</p>
          </div>

          {/* Manager */}
          <div>
            <p className="mb-2 text-xs font-semibold uppercase tracking-[0.2em] text-slate-500">Manager</p>
            <div className="grid gap-3 sm:grid-cols-2">
              <input
                autoFocus
                className={INPUT}
                value={mgrName}
                onChange={(e) => setMgrName(e.target.value)}
                placeholder="Manager name *"
                disabled={creating}
              />
              <input
                className={INPUT}
                value={mgrModel}
                onChange={(e) => setMgrModel(e.target.value)}
                placeholder="Model (gpt-4o-mini)"
                disabled={creating}
              />
            </div>
          </div>

          {/* Workers */}
          <div>
            <div className="mb-2 flex items-center justify-between">
              <p className="text-xs font-semibold uppercase tracking-[0.2em] text-slate-500">Workers</p>
              <button
                type="button"
                onClick={() => setWorkers((p) => [...p, { name: '', model: '' }])}
                disabled={creating}
                className="rounded-full border border-slate-200 px-2.5 py-1 text-xs font-medium text-slate-600 transition hover:bg-slate-50"
              >
                + Add worker
              </button>
            </div>
            <div className="space-y-2">
              {workers.map((w, i) => (
                <div key={i} className="flex items-center gap-2">
                  <input
                    className={`${INPUT} flex-1`}
                    value={w.name}
                    onChange={(e) => updateWorker(i, 'name', e.target.value)}
                    placeholder={`Worker ${i + 1} name`}
                    disabled={creating}
                  />
                  <input
                    className={`${INPUT} w-36`}
                    value={w.model}
                    onChange={(e) => updateWorker(i, 'model', e.target.value)}
                    placeholder="gpt-4o-mini"
                    disabled={creating}
                  />
                  {workers.length > 1 && (
                    <button
                      type="button"
                      onClick={() => setWorkers((p) => p.filter((_, idx) => idx !== i))}
                      disabled={creating}
                      className="rounded-full p-1.5 text-slate-400 transition hover:bg-rose-50 hover:text-rose-500"
                    >
                      <svg
                        className="h-3.5 w-3.5"
                        viewBox="0 0 16 16"
                        fill="none"
                        stroke="currentColor"
                        strokeWidth="2"
                      >
                        <path d="M2 2l12 12M14 2L2 14" strokeLinecap="round" />
                      </svg>
                    </button>
                  )}
                </div>
              ))}
            </div>
            <p className="mt-2 text-xs text-slate-400">
              Leave name empty to auto-generate a worker from the manager name.
            </p>
          </div>

          {/* Limits */}
          <div className="grid gap-3 sm:grid-cols-2">
            <Field label="Max turns" hint="Optional">
              <input
                className={INPUT}
                type="number"
                min="1"
                value={maxTurns}
                onChange={(e) => setMaxTurns(e.target.value)}
                placeholder="e.g. 10"
                disabled={creating}
              />
            </Field>
            <Field label="Token budget" hint="Optional">
              <input
                className={INPUT}
                type="number"
                min="1"
                value={tokenBudget}
                onChange={(e) => setTokenBudget(e.target.value)}
                placeholder="e.g. 4000"
                disabled={creating}
              />
            </Field>
          </div>

          {error && <ErrorBox>{error}</ErrorBox>}

          <ModalActions onClose={onClose} closeable={!creating}>
            <SubmitButton disabled={creating || !mgrName.trim()} loading={creating} accent="emerald">
              Create swarm
            </SubmitButton>
          </ModalActions>
        </form>
      </div>
    </Modal>
  );
}

// ── Run Modal (agent or swarm) ────────────────────────────────────────────────
interface RunResult {
  status: string;
  durationMs?: number;
  output: string | null;
  error: string | null;
}

function RunModal({
  kind,
  label,
  onClose,
  onRun,
}: {
  kind: 'agent' | 'swarm';
  label: string;
  onClose: () => void;
  onRun: (task: string) => Promise<RunResult>;
}) {
  const [task, setTask] = useState('');
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<RunResult | null>(null);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!task.trim()) return;
    setRunning(true);
    setError(null);
    try {
      setResult(await onRun(task.trim()));
      setTask('');
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setRunning(false);
    }
  }

  const isAgent = kind === 'agent';
  const badge = isAgent ? 'bg-violet-100 text-violet-700' : 'bg-sky-100 text-sky-700';
  const accent: ButtonAccent = isAgent ? 'violet' : 'sky';

  return (
    <Modal onClose={onClose}>
      <div className="px-6 py-6 sm:px-8">
        <div className="mb-6 flex items-start justify-between gap-4">
          <div>
            <p className={`mb-1 inline-flex rounded-full px-2.5 py-0.5 text-xs font-semibold uppercase tracking-[0.2em] ${badge}`}>
              Run {kind}
            </p>
            <h2 className="text-xl font-semibold text-slate-950">{label}</h2>
            <p className="mt-1 text-sm text-slate-500">Send a task and wait for the daemon response.</p>
          </div>
          <ModalClose onClose={onClose} />
        </div>

        <form onSubmit={handleSubmit} className="space-y-4">
          <textarea
            autoFocus
            className={`${INPUT} min-h-[8rem] resize-none`}
            value={task}
            onChange={(e) => setTask(e.target.value)}
            placeholder={
              isAgent
                ? 'Ask the agent to summarise, plan, research, or respond to anything…'
                : 'Describe the coordination task for the swarm to execute…'
            }
            disabled={running}
          />

          {error && <ErrorBox>{error}</ErrorBox>}

          {result && (
            <div
              className={[
                'rounded-xl border px-4 py-4',
                result.status === 'success'
                  ? 'border-emerald-200 bg-emerald-50'
                  : 'border-rose-200 bg-rose-50',
              ].join(' ')}
            >
              <div className="flex items-center justify-between gap-3">
                <span className="text-sm font-semibold text-slate-900">Result</span>
                <span
                  className={[
                    'rounded-full px-2.5 py-1 text-xs font-semibold uppercase tracking-[0.16em]',
                    result.status === 'success'
                      ? 'bg-emerald-200 text-emerald-800'
                      : 'bg-rose-200 text-rose-800',
                  ].join(' ')}
                >
                  {result.status}
                </span>
              </div>
              {result.durationMs !== undefined && (
                <p className="mt-1 text-xs text-slate-400">
                  {(result.durationMs / 1000).toFixed(2)}s
                </p>
              )}
              {result.output && (
                <pre className="mt-3 max-h-60 overflow-y-auto whitespace-pre-wrap rounded-xl bg-white/80 px-3 py-3 font-mono text-xs leading-6 text-slate-700">
                  {result.output}
                </pre>
              )}
              {result.error && (
                <p className="mt-2 text-sm text-rose-700">{result.error}</p>
              )}
            </div>
          )}

          <ModalActions onClose={onClose} closeable>
            <SubmitButton disabled={running || !task.trim()} loading={running} accent={accent}>
              Run
            </SubmitButton>
          </ModalActions>
        </form>
      </div>
    </Modal>
  );
}

// ── App ───────────────────────────────────────────────────────────────────────
// ── Login ─────────────────────────────────────────────────────────────────────
function LoginPage({ onEnter }: { onEnter: () => void }) {
  const [email, setEmail] = useState('');
  const [pass, setPass] = useState('');

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    onEnter();
  }

  return (
    <div className="flex min-h-screen">
      {/* ── Brand panel ── */}
      <div className="hidden w-[52%] flex-col justify-between bg-slate-950 p-12 lg:flex">
        <div>
          <span className="inline-flex items-center gap-2 rounded-full border border-white/10 bg-white/5 px-3.5 py-1.5 text-xs font-semibold uppercase tracking-[0.26em] text-white/60">
            animaOS
          </span>
        </div>

        <div>
          <h1 className="text-4xl font-semibold leading-tight text-white">
            The runtime for
            <br />
            autonomous agents.
          </h1>
          <p className="mt-4 text-base text-white/50">
            Manage swarms, memory, and live inference from one control room.
          </p>

          <ul className="mt-10 space-y-4">
            {[
              { icon: '⬡', text: 'Multi-agent swarms with supervisor, dynamic, or round-robin coordination' },
              { icon: '◈', text: 'Persistent vector memory — search, score, and replay past context' },
              { icon: '◎', text: 'Live SSE event streaming with real-time task execution tracing' },
            ].map(({ icon, text }) => (
              <li key={text} className="flex items-start gap-3 text-sm text-white/50">
                <span className="mt-0.5 shrink-0 text-orange-400">{icon}</span>
                {text}
              </li>
            ))}
          </ul>
        </div>

        <p className="text-xs text-white/20">animaOS · runtime console</p>
      </div>

      {/* ── Form panel ── */}
      <div className="flex flex-1 flex-col items-center justify-center px-6 py-16">
        {/* Mobile logo */}
        <div className="mb-8 lg:hidden">
          <span className="text-sm font-semibold tracking-tight text-slate-900">animaOS</span>
        </div>

        <div className="w-full max-w-sm">
          <h2 className="text-2xl font-semibold text-slate-950">Sign in</h2>
          <p className="mt-1.5 text-sm text-slate-500">Access the control room.</p>

          <form onSubmit={handleSubmit} className="mt-8 space-y-4">
            <div className="space-y-1.5">
              <label className="block text-xs font-semibold uppercase tracking-[0.2em] text-slate-500">
                Email
              </label>
              <input
                type="email"
                className={INPUT}
                value={email}
                onChange={(e) => setEmail(e.target.value)}
                placeholder="you@example.com"
                autoComplete="email"
              />
            </div>

            <div className="space-y-1.5">
              <label className="block text-xs font-semibold uppercase tracking-[0.2em] text-slate-500">
                Password
              </label>
              <input
                type="password"
                className={INPUT}
                value={pass}
                onChange={(e) => setPass(e.target.value)}
                placeholder="••••••••"
                autoComplete="current-password"
              />
            </div>

            <button
              type="submit"
              className="mt-2 w-full rounded-xl bg-slate-950 py-3 text-sm font-semibold text-white transition hover:bg-slate-800"
            >
              Sign in
            </button>
          </form>

          <div className="mt-8 flex items-center gap-3">
            <div className="h-px flex-1 bg-slate-100" />
            <span className="text-xs text-slate-400">or</span>
            <div className="h-px flex-1 bg-slate-100" />
          </div>

          <button
            type="button"
            onClick={onEnter}
            className="mt-6 w-full rounded-xl border border-slate-200 py-3 text-sm font-medium text-slate-600 transition hover:bg-slate-50 hover:text-slate-800"
          >
            Continue without signing in
          </button>

          <p className="mt-4 text-center text-xs text-slate-400">
            Dev bypass — no credentials required in local mode.
          </p>
        </div>
      </div>
    </div>
  );
}

// ── App root ──────────────────────────────────────────────────────────────────
export function App() {
  const [loggedIn, setLoggedIn] = useState(false);

  if (!loggedIn) return <LoginPage onEnter={() => setLoggedIn(true)} />;
  return <Dashboard onSignOut={() => setLoggedIn(false)} />;
}

// ── Tab type ──────────────────────────────────────────────────────────────────
type Tab = 'overview' | 'agents' | 'swarms' | 'memory';

// ── Dashboard ─────────────────────────────────────────────────────────────────
function Dashboard({ onSignOut }: { onSignOut: () => void }) {
  const [tab, setTab] = useState<Tab>('overview');

  // Data
  const [healthState, setHealthState] = useState<HealthResponse | null>(null);
  const [providerList, setProviderList] = useState<ProviderResponse[]>([]);
  const [agentSnapshots, setAgentSnapshots] = useState<AgentSnapshot[]>([]);
  const [swarmStates, setSwarmStates] = useState<SwarmState[]>([]);
  const [recentMemories, setRecentMemories] = useState<Memory[]>([]);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [globalError, setGlobalError] = useState<string | null>(null);
  const [refreshNonce, setRefreshNonce] = useState(0);

  // Modals
  const [showNewAgent, setShowNewAgent] = useState(false);
  const [showNewSwarm, setShowNewSwarm] = useState(false);
  const [runAgentTarget, setRunAgentTarget] = useState<AgentSnapshot | null>(null);
  const [runSwarmTarget, setRunSwarmTarget] = useState<SwarmState | null>(null);

  // Actions
  const [deletingAgentId, setDeletingAgentId] = useState<string | null>(null);
  const [streamingSwarmId, setStreamingSwarmId] = useState<string | null>(null);
  const [liveEvents, setLiveEvents] = useState<SwarmStreamEvent[]>([]);
  const streamCleanupRef = useRef<(() => void) | null>(null);

  // Memory search
  const [memQuery, setMemQuery] = useState('');
  const [memResults, setMemResults] = useState<MemorySearchResult[]>([]);
  const [searchingMem, setSearchingMem] = useState(false);

  useEffect(() => {
    let active = true;
    const load = async (bg = false) => {
      bg ? setRefreshing(true) : setLoading(true);
      try {
        const [h, p, a, sw, mem] = await Promise.all([
          health.get(),
          providerApi.list(),
          agents.list(),
          swarms.list(),
          memories.recent({ limit: 12 }),
        ]);
        if (!active) return;
        setHealthState(h);
        setProviderList(p);
        setAgentSnapshots(a);
        setSwarmStates(sw);
        setRecentMemories(mem);
        setGlobalError(null);
      } catch (err) {
        if (!active) return;
        setGlobalError(err instanceof Error ? err.message : String(err));
      } finally {
        if (!active) return;
        setLoading(false);
        setRefreshing(false);
      }
    };
    void load();
    const id = window.setInterval(() => void load(true), 10_000);
    return () => {
      active = false;
      window.clearInterval(id);
    };
  }, [refreshNonce]);

  useEffect(() => () => { streamCleanupRef.current?.(); }, []);

  const configuredProviders = useMemo(() => providerList.filter((p) => p.configured), [providerList]);
  const totalMsgs = useMemo(() => swarmStates.reduce((n, s) => n + s.messages.length, 0), [swarmStates]);
  const daemonOnline = healthState?.status === 'ok';
  const refresh = () => setRefreshNonce((n) => n + 1);

  async function handleDeleteAgent(agentId: string) {
    setDeletingAgentId(agentId);
    try {
      await agents.delete(agentId);
      refresh();
    } catch (err) {
      setGlobalError(err instanceof Error ? err.message : String(err));
    } finally {
      setDeletingAgentId(null);
    }
  }

  function handleToggleStream(swarmId: string) {
    if (streamingSwarmId === swarmId) {
      streamCleanupRef.current?.();
      streamCleanupRef.current = null;
      setStreamingSwarmId(null);
      setLiveEvents([]);
      return;
    }
    streamCleanupRef.current?.();
    setLiveEvents([]);
    setStreamingSwarmId(swarmId);
    streamCleanupRef.current = swarms.streamEvents(
      swarmId,
      (ev) => setLiveEvents((prev) => [ev, ...prev].slice(0, 60)),
      () => {
        setStreamingSwarmId(null);
        streamCleanupRef.current = null;
      }
    );
  }

  async function runAgentTask(agentId: string, task: string): Promise<RunResult> {
    const resp = await agents.run(agentId, task);
    refresh();
    return {
      status: resp.result.status,
      durationMs: resp.result.durationMs,
      output: formatTaskOutput(resp.result.data),
      error: resp.result.error ?? null,
    };
  }

  async function runSwarmTask(swarmId: string, task: string): Promise<RunResult> {
    const resp = await swarms.run(swarmId, task);
    refresh();
    return {
      status: resp.result.status,
      durationMs: resp.result.durationMs,
      output: formatTaskOutput(resp.result.data),
      error: resp.result.error ?? null,
    };
  }

  async function handleSearchMemories() {
    if (!memQuery.trim()) return;
    setSearchingMem(true);
    try {
      setMemResults(await memories.search({ q: memQuery.trim(), limit: 12 }));
    } catch (err) {
      setGlobalError(err instanceof Error ? err.message : String(err));
    } finally {
      setSearchingMem(false);
    }
  }

  // ── Nav tabs config
  const tabs: { id: Tab; label: string; count?: number }[] = [
    { id: 'overview', label: 'Overview' },
    { id: 'agents', label: 'Agents', count: agentSnapshots.length },
    { id: 'swarms', label: 'Swarms', count: swarmStates.length },
    { id: 'memory', label: 'Memory' },
  ];

  return (
    <>
      <div className="min-h-screen bg-slate-50 text-slate-950">

        {/* ── Top bar ── */}
        <header className="sticky top-0 z-40 border-b border-slate-200 bg-white">
          <div className="mx-auto flex max-w-6xl items-center justify-between gap-4 px-5 py-0">

            {/* Left: logo + tabs */}
            <div className="flex items-stretch gap-1">
              <div className="flex items-center gap-2 border-r border-slate-100 pr-5 mr-1">
                <span className="text-sm font-semibold tracking-tight text-slate-950">animaOS</span>
                {/* Daemon dot */}
                <span
                  title={daemonOnline ? 'Daemon online' : 'Daemon offline'}
                  className={[
                    'h-1.5 w-1.5 rounded-full',
                    daemonOnline ? 'bg-emerald-500' : 'bg-slate-300',
                  ].join(' ')}
                />
              </div>

              <nav className="flex items-stretch gap-0.5">
                {tabs.map((t) => (
                  <button
                    key={t.id}
                    onClick={() => setTab(t.id)}
                    className={[
                      'flex items-center gap-1.5 border-b-2 px-4 py-4 text-sm font-medium transition',
                      tab === t.id
                        ? 'border-slate-950 text-slate-950'
                        : 'border-transparent text-slate-500 hover:text-slate-800',
                    ].join(' ')}
                  >
                    {t.label}
                    {t.count !== undefined && t.count > 0 && (
                      <span className="rounded-full bg-slate-100 px-1.5 py-0.5 text-xs font-semibold text-slate-600">
                        {t.count}
                      </span>
                    )}
                  </button>
                ))}
              </nav>
            </div>

            {/* Right: actions */}
            <div className="flex items-center gap-2">
              <button
                onClick={() => setShowNewAgent(true)}
                className="rounded-lg bg-orange-600 px-3.5 py-2 text-xs font-semibold text-white transition hover:bg-orange-500"
              >
                + Agent
              </button>
              <button
                onClick={() => setShowNewSwarm(true)}
                className="rounded-lg bg-emerald-600 px-3.5 py-2 text-xs font-semibold text-white transition hover:bg-emerald-500"
              >
                + Swarm
              </button>
              <button
                onClick={refresh}
                disabled={loading || refreshing}
                className="rounded-lg border border-slate-200 px-3.5 py-2 text-xs font-medium text-slate-600 transition hover:bg-slate-50 disabled:opacity-40"
              >
                {refreshing ? '…' : 'Refresh'}
              </button>
              <button
                onClick={onSignOut}
                className="rounded-lg px-3 py-2 text-xs font-medium text-slate-400 transition hover:text-slate-700"
              >
                Sign out
              </button>
            </div>
          </div>
        </header>

        {/* ── Content ── */}
        <main className="mx-auto max-w-6xl px-5 py-8">
          {/* Error banner */}
          {globalError && (
            <div className="mb-6 flex items-center justify-between gap-3 rounded-xl border border-rose-200 bg-rose-50 px-4 py-3 text-sm text-rose-700">
              <span>{globalError}</span>
              <button
                onClick={() => setGlobalError(null)}
                className="shrink-0 rounded-full p-1 text-rose-400 hover:bg-rose-100"
              >
                <svg className="h-3.5 w-3.5" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="2">
                  <path d="M2 2l12 12M14 2L2 14" strokeLinecap="round" />
                </svg>
              </button>
            </div>
          )}

          {/* ── Overview tab ── */}
          {tab === 'overview' && (
            <div className="space-y-6">
              {/* Stat row */}
              <div className="grid grid-cols-2 gap-4 sm:grid-cols-4">
                <StatCard
                  label="Daemon"
                  value={loading ? '—' : daemonOnline ? 'Online' : 'Offline'}
                  sub={healthState ? `v${healthState.version ?? '?'}` : undefined}
                  accent={daemonOnline ? 'emerald' : 'rose'}
                />
                <StatCard
                  label="Providers"
                  value={`${configuredProviders.length} / ${providerList.length}`}
                  sub="configured"
                />
                <StatCard
                  label="Agents"
                  value={String(agentSnapshots.length)}
                  sub="registered"
                />
                <StatCard
                  label="Swarms"
                  value={String(swarmStates.length)}
                  sub={`${totalMsgs} msgs total`}
                />
              </div>

              {/* Two columns */}
              <div className="grid gap-5 lg:grid-cols-2">
                {/* Providers */}
                <Card title="Providers">
                  {providerList.length ? (
                    <div className="divide-y divide-slate-100">
                      {providerList.map((p) => (
                        <div key={p.id} className="flex items-center justify-between gap-2 py-3">
                          <div>
                            <p className="text-sm font-medium text-slate-900">{p.label}</p>
                            <p className="text-xs text-slate-400">{p.id}</p>
                          </div>
                          <StatusBadge ok={p.configured} okLabel="Ready" failLabel="Missing" />
                        </div>
                      ))}
                    </div>
                  ) : (
                    <Empty message={loading ? 'Loading…' : 'No providers'} />
                  )}
                </Card>

                {/* Health */}
                <Card title="Runtime">
                  {healthState ? (
                    <div className="space-y-4">
                      <div className="flex items-center justify-between">
                        <span className="text-sm text-slate-500">Uptime</span>
                        <span className="font-mono text-sm font-semibold text-slate-900">
                          {formatDuration(healthState.uptime_secs)}
                        </span>
                      </div>
                      <div className="flex items-center justify-between">
                        <span className="text-sm text-slate-500">Status</span>
                        <StatusBadge ok={daemonOnline} okLabel="Running" failLabel="Error" />
                      </div>
                      <div className="flex items-center justify-between">
                        <span className="text-sm text-slate-500">Swarm messages</span>
                        <span className="font-mono text-sm font-semibold text-slate-900">{totalMsgs}</span>
                      </div>
                    </div>
                  ) : (
                    <Empty message={loading ? 'Loading…' : 'Daemon unreachable'} />
                  )}
                </Card>
              </div>
            </div>
          )}

          {/* ── Agents tab ── */}
          {tab === 'agents' && (
            <Card title="Agents" count={agentSnapshots.length}>
              {agentSnapshots.length ? (
                <div className="divide-y divide-slate-100">
                  {agentSnapshots.map((agent) => (
                    <div
                      key={agent.state.id}
                      className="flex items-center justify-between gap-4 py-4"
                    >
                      <div className="min-w-0 flex-1">
                        <div className="flex items-center gap-2">
                          <p className="font-medium text-slate-900">{agent.state.name}</p>
                          <span className="rounded-full bg-slate-100 px-2 py-0.5 text-xs font-semibold text-slate-600">
                            {agent.state.status}
                          </span>
                        </div>
                        <p className="mt-0.5 text-xs text-slate-400">
                          {agent.state.config?.model ?? '—'} · {agent.state.config?.provider ?? '—'} · {agent.messageCount} msgs
                        </p>
                        {agent.state.config?.bio && (
                          <p className="mt-1 text-xs text-slate-500 line-clamp-1">{agent.state.config.bio}</p>
                        )}
                      </div>
                      <div className="flex shrink-0 items-center gap-2">
                        <button
                          onClick={() => setRunAgentTarget(agent)}
                          className="rounded-lg bg-violet-50 border border-violet-200 px-3 py-1.5 text-xs font-semibold text-violet-700 transition hover:bg-violet-100"
                        >
                          Run
                        </button>
                        <button
                          onClick={() => handleDeleteAgent(agent.state.id)}
                          disabled={deletingAgentId === agent.state.id}
                          className="rounded-lg border border-rose-100 px-3 py-1.5 text-xs font-semibold text-rose-500 transition hover:bg-rose-50 disabled:opacity-40"
                        >
                          {deletingAgentId === agent.state.id ? '…' : 'Delete'}
                        </button>
                      </div>
                    </div>
                  ))}
                </div>
              ) : (
                <Empty message="No agents yet. Create one with + Agent." />
              )}
            </Card>
          )}

          {/* ── Swarms tab ── */}
          {tab === 'swarms' && (
            <div className="space-y-5">
              <Card title="Swarms" count={swarmStates.length}>
                {swarmStates.length ? (
                  <div className="divide-y divide-slate-100">
                    {swarmStates.map((swarm) => (
                      <div key={swarm.id} className="py-4">
                        <div className="flex items-center justify-between gap-4">
                          <div className="min-w-0 flex-1">
                            <div className="flex items-center gap-2">
                              <p className="font-mono text-sm font-medium text-slate-900 truncate">{swarm.id}</p>
                              <span className="rounded-full bg-sky-100 px-2 py-0.5 text-xs font-semibold text-sky-700 shrink-0">
                                {swarm.status}
                              </span>
                            </div>
                            <p className="mt-0.5 text-xs text-slate-400">
                              {swarm.agentIds.length} agents · {swarm.messages.length} msgs
                            </p>
                          </div>
                          <div className="flex shrink-0 items-center gap-2">
                            <button
                              onClick={() => setRunSwarmTarget(swarm)}
                              className="rounded-lg bg-sky-50 border border-sky-200 px-3 py-1.5 text-xs font-semibold text-sky-700 transition hover:bg-sky-100"
                            >
                              Run
                            </button>
                            <button
                              onClick={() => handleToggleStream(swarm.id)}
                              className={[
                                'rounded-lg border px-3 py-1.5 text-xs font-semibold transition',
                                streamingSwarmId === swarm.id
                                  ? 'border-rose-200 bg-rose-50 text-rose-700 hover:bg-rose-100'
                                  : 'border-emerald-200 bg-emerald-50 text-emerald-700 hover:bg-emerald-100',
                              ].join(' ')}
                            >
                              {streamingSwarmId === swarm.id ? 'Stop stream' : 'Live events'}
                            </button>
                          </div>
                        </div>

                        {/* Live event feed for this swarm */}
                        {streamingSwarmId === swarm.id && (
                          <div className="mt-3">
                            {liveEvents.length > 0 ? (
                              <div className="max-h-52 space-y-1 overflow-y-auto rounded-xl border border-emerald-200 bg-emerald-50 p-3">
                                {liveEvents.map((ev, i) => (
                                  <div key={i} className="flex items-start gap-2 text-xs">
                                    <span className="shrink-0 rounded bg-emerald-200 px-1.5 py-0.5 font-mono font-semibold text-emerald-800">
                                      {ev.event}
                                    </span>
                                    <span className="break-all font-mono text-slate-600">
                                      {'agentName' in ev.data
                                        ? (ev.data as { agentName: string }).agentName
                                        : JSON.stringify(ev.data).slice(0, 80)}
                                    </span>
                                  </div>
                                ))}
                              </div>
                            ) : (
                              <div className="flex items-center gap-2 rounded-xl border border-emerald-200 bg-emerald-50 px-3 py-2.5 text-xs text-emerald-700">
                                <span className="relative flex h-2 w-2">
                                  <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-emerald-400 opacity-75" />
                                  <span className="relative inline-flex h-2 w-2 rounded-full bg-emerald-500" />
                                </span>
                                Listening for events…
                              </div>
                            )}
                          </div>
                        )}
                      </div>
                    ))}
                  </div>
                ) : (
                  <Empty message="No swarms yet. Create one with + Swarm." />
                )}
              </Card>
            </div>
          )}

          {/* ── Memory tab ── */}
          {tab === 'memory' && (
            <div className="space-y-5">
              {/* Search */}
              <div className="flex items-center gap-3">
                <input
                  className="flex-1 rounded-xl border border-slate-200 bg-white px-4 py-3 text-sm text-slate-900 outline-none transition focus:border-orange-300 focus:ring-2 focus:ring-orange-200"
                  value={memQuery}
                  onChange={(e) => setMemQuery(e.target.value)}
                  onKeyDown={(e) => e.key === 'Enter' && void handleSearchMemories()}
                  placeholder="Search memories by content or agent…"
                  disabled={searchingMem}
                />
                <button
                  onClick={handleSearchMemories}
                  disabled={searchingMem || !memQuery.trim()}
                  className="rounded-xl bg-slate-950 px-5 py-3 text-sm font-semibold text-white transition hover:bg-slate-800 disabled:opacity-40"
                >
                  {searchingMem ? 'Searching…' : 'Search'}
                </button>
                {memResults.length > 0 && (
                  <button
                    onClick={() => { setMemResults([]); setMemQuery(''); }}
                    className="rounded-xl border border-slate-200 px-4 py-3 text-sm text-slate-500 transition hover:bg-slate-50"
                  >
                    Clear
                  </button>
                )}
              </div>

              {/* Results or recent */}
              <Card
                title={memResults.length > 0 ? `Search results` : 'Recent memories'}
                count={memResults.length > 0 ? memResults.length : recentMemories.length}
              >
                {(memResults.length > 0 ? memResults : recentMemories).length ? (
                  <div className="divide-y divide-slate-100">
                    {(memResults.length > 0 ? memResults : recentMemories).map((m) => (
                      <div key={m.id} className="py-4">
                        <div className="flex items-start justify-between gap-3">
                          <div className="min-w-0 flex-1">
                            <div className="flex items-center gap-2">
                              <p className="text-xs font-semibold text-slate-700">{m.agentName}</p>
                              <span className="rounded bg-orange-100 px-1.5 py-0.5 text-xs font-semibold text-orange-700">
                                {m.importance.toFixed(1)}
                              </span>
                            </div>
                            <p className="mt-1.5 text-sm text-slate-700 line-clamp-2">{m.content}</p>
                          </div>
                          <p className="shrink-0 text-xs text-slate-400">{formatTimestamp(m.createdAt)}</p>
                        </div>
                      </div>
                    ))}
                  </div>
                ) : (
                  <Empty
                    message={
                      searchingMem
                        ? 'Searching…'
                        : memQuery
                        ? 'No results found.'
                        : 'No memories stored yet.'
                    }
                  />
                )}
              </Card>
            </div>
          )}
        </main>
      </div>

      {/* ── Modals ── */}
      {showNewAgent && (
        <NewAgentModal
          configuredProviders={configuredProviders}
          onClose={() => setShowNewAgent(false)}
          onCreated={() => { setShowNewAgent(false); refresh(); }}
        />
      )}
      {showNewSwarm && (
        <NewSwarmModal
          configuredProviders={configuredProviders}
          onClose={() => setShowNewSwarm(false)}
          onCreated={() => { setShowNewSwarm(false); refresh(); }}
        />
      )}
      {runAgentTarget && (
        <RunModal
          kind="agent"
          label={runAgentTarget.state.name}
          onClose={() => setRunAgentTarget(null)}
          onRun={(task) => runAgentTask(runAgentTarget.state.id, task)}
        />
      )}
      {runSwarmTarget && (
        <RunModal
          kind="swarm"
          label={runSwarmTarget.id}
          onClose={() => setRunSwarmTarget(null)}
          onRun={(task) => runSwarmTask(runSwarmTarget.id, task)}
        />
      )}
    </>
  );
}

// ── Shared UI helpers ─────────────────────────────────────────────────────────

function StatCard({
  label,
  value,
  sub,
  accent,
}: {
  label: string;
  value: string;
  sub?: string;
  accent?: 'emerald' | 'rose';
}) {
  return (
    <div className="rounded-2xl border border-slate-200 bg-white p-5">
      <p className="text-xs font-semibold uppercase tracking-[0.2em] text-slate-400">{label}</p>
      <p
        className={[
          'mt-2 text-2xl font-semibold',
          accent === 'emerald' ? 'text-emerald-600' : accent === 'rose' ? 'text-rose-600' : 'text-slate-950',
        ].join(' ')}
      >
        {value}
      </p>
      {sub && <p className="mt-0.5 text-xs text-slate-400">{sub}</p>}
    </div>
  );
}

function Card({
  title,
  count,
  children,
}: {
  title?: string;
  count?: number;
  children: React.ReactNode;
}) {
  return (
    <div className="rounded-2xl border border-slate-200 bg-white p-6">
      {title && (
        <div className="mb-4 flex items-center gap-2">
          <h2 className="text-sm font-semibold text-slate-900">{title}</h2>
          {count !== undefined && (
            <span className="rounded-full bg-slate-100 px-2 py-0.5 text-xs font-semibold text-slate-500">
              {count}
            </span>
          )}
        </div>
      )}
      {children}
    </div>
  );
}

function StatusBadge({
  ok,
  okLabel,
  failLabel,
}: {
  ok: boolean;
  okLabel: string;
  failLabel: string;
}) {
  return (
    <span
      className={[
        'rounded-full px-2.5 py-1 text-xs font-semibold',
        ok ? 'bg-emerald-100 text-emerald-700' : 'bg-slate-100 text-slate-500',
      ].join(' ')}
    >
      {ok ? okLabel : failLabel}
    </span>
  );
}

function Empty({ message }: { message: string }) {
  return (
    <p className="rounded-xl border border-dashed border-slate-200 py-8 text-center text-sm text-slate-400">
      {message}
    </p>
  );
}

function formatDuration(totalSeconds: number) {
  if (totalSeconds < 60) return `${Math.round(totalSeconds)}s`;
  if (totalSeconds < 3600) return `${Math.round(totalSeconds / 60)}m`;
  return `${(totalSeconds / 3600).toFixed(1)}h`;
}

function formatTimestamp(timestamp: number) {
  return new Intl.DateTimeFormat(undefined, {
    month: 'short',
    day: 'numeric',
    hour: 'numeric',
    minute: '2-digit',
  }).format(timestamp);
}

function formatTaskOutput(data: unknown): string | null {
  if (typeof data === 'string') return data;
  if (data && typeof data === 'object') {
    if ('text' in data && typeof (data as { text: unknown }).text === 'string')
      return (data as { text: string }).text;
    return JSON.stringify(data, null, 2);
  }
  return null;
}

export default App;
