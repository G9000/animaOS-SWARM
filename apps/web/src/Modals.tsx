import React, { useEffect, useState } from 'react';
import {
  agents, swarms, type AgentConfig, type AgentSnapshot,
  type ProviderResponse, type SwarmCreateRequest, type SwarmState,
  type TaskResult, type WorkerConfig,
} from './lib/api';
import { Colors, MONO } from './design';

// ── Shared form primitives ────────────────────────────────────────────────────
const inputStyle = (c: Colors): React.CSSProperties => ({
  width: '100%', padding: '9px 12px', fontSize: 13,
  background: c.elevated, color: c.textPrimary,
  border: `1px solid ${c.border}`, outline: 'none',
  fontFamily: 'inherit', transition: 'border-color 0.15s',
});

function Field({ label, hint, required, children }: {
  label: string; hint?: string; required?: boolean; children: React.ReactNode;
}) {
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 5 }}>
      <label style={{ fontSize: 9, fontFamily: MONO, letterSpacing: 1.4, textTransform: 'uppercase', color: 'inherit', display: 'flex', gap: 4 }}>
        {label}
        {required && <span style={{ color: '#f87171' }}>*</span>}
        {hint && <span style={{ fontWeight: 400, textTransform: 'none', letterSpacing: 0, opacity: 0.6 }}>— {hint}</span>}
      </label>
      {children}
    </div>
  );
}

function ErrorBox({ children }: { children: React.ReactNode }) {
  return (
    <div style={{ padding: '10px 14px', fontSize: 13, color: '#f87171', border: '1px solid rgba(248,113,113,0.3)', background: 'rgba(248,113,113,0.07)' }}>
      {children}
    </div>
  );
}

// ── Modal backdrop ────────────────────────────────────────────────────────────
export function Modal({ onClose, children, c }: { onClose: () => void; children: React.ReactNode; c: Colors }) {
  useEffect(() => {
    const h = (e: KeyboardEvent) => { if (e.key === 'Escape') onClose(); };
    window.addEventListener('keydown', h);
    return () => window.removeEventListener('keydown', h);
  }, [onClose]);

  return (
    <div role="dialog" aria-modal="true"
      style={{ position: 'fixed', inset: 0, zIndex: 100, display: 'flex', alignItems: 'center', justifyContent: 'center', padding: 24, background: 'rgba(0,0,0,0.6)', backdropFilter: 'blur(4px)' }}
      onClick={e => { if (e.target === e.currentTarget) onClose(); }}>
      <div style={{ width: '100%', maxWidth: 520, maxHeight: '90vh', overflowY: 'auto', background: c.elevated, border: `1px solid ${c.border}`, boxShadow: '0 40px 80px rgba(0,0,0,0.5)' }}>
        {children}
      </div>
    </div>
  );
}

function ModalHeader({ label, title, sub, onClose, c, labelColor = c.accent }: {
  label: string; title: string; sub?: string; onClose: () => void; c: Colors; labelColor?: string;
}) {
  return (
    <div style={{ padding: '24px 28px 20px', borderBottom: `1px solid ${c.border}`, display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start', gap: 16 }}>
      <div>
        <div style={{ fontSize: 9, fontFamily: MONO, letterSpacing: 1.6, textTransform: 'uppercase', color: labelColor, marginBottom: 6 }}>{label}</div>
        <div style={{ fontSize: 18, fontWeight: 700 }}>{title}</div>
        {sub && <div style={{ fontSize: 12, color: c.textMuted, marginTop: 3 }}>{sub}</div>}
      </div>
      <button onClick={onClose} style={{ background: 'transparent', border: 'none', color: c.textMuted, cursor: 'pointer', fontSize: 18, lineHeight: 1, padding: 4 }}>✕</button>
    </div>
  );
}

function SubmitBtn({ disabled, loading, accent, children }: {
  disabled: boolean; loading: boolean; accent: string; children: React.ReactNode;
}) {
  return (
    <button type="submit" disabled={disabled}
      style={{ padding: '10px 20px', fontSize: 13, fontWeight: 600, color: '#fff', background: disabled ? '#333' : accent, border: 'none', cursor: disabled ? 'not-allowed' : 'pointer', opacity: disabled ? 0.6 : 1, transition: 'background 0.15s' }}>
      {loading ? 'Working…' : children}
    </button>
  );
}

// ── New Agent Modal ───────────────────────────────────────────────────────────
export function NewAgentModal({ configuredProviders, onClose, onCreated, c }: {
  configuredProviders: ProviderResponse[];
  onClose: () => void;
  onCreated: (agent: AgentSnapshot) => void;
  c: Colors;
}) {
  const [name, setName] = useState('');
  const [model, setModel] = useState('');
  const [provider, setProvider] = useState(configuredProviders[0]?.id ?? '');
  const [bio, setBio] = useState('');
  const [system, setSystem] = useState('');
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const IS = inputStyle(c);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!name.trim()) return;
    setCreating(true); setError(null);
    try {
      const config: AgentConfig = {
        name: name.trim(), model: model.trim() || 'gpt-4o-mini',
        provider: provider || configuredProviders[0]?.id || 'openai',
        ...(bio.trim() ? { bio: bio.trim() } : {}),
        ...(system.trim() ? { system: system.trim() } : {}),
      };
      onCreated(await agents.create(config));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally { setCreating(false); }
  }

  return (
    <Modal onClose={onClose} c={c}>
      <ModalHeader label="New agent" title="Create an agent" sub="Registers a new agent on the running daemon." onClose={onClose} c={c} labelColor={c.accent} />
      <form onSubmit={handleSubmit} style={{ padding: '24px 28px', display: 'flex', flexDirection: 'column', gap: 16 }}>
        <Field label="Name" required>
          <input autoFocus style={IS} value={name} onChange={e => setName(e.target.value)} placeholder="e.g. Research assistant" disabled={creating} />
        </Field>
        <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 12 }}>
          <Field label="Provider">
            {configuredProviders.length ? (
              <select style={{ ...IS, cursor: 'pointer' }} value={provider} onChange={e => setProvider(e.target.value)} disabled={creating}>
                {configuredProviders.map(p => <option key={p.id} value={p.id}>{p.label}</option>)}
              </select>
            ) : (
              <input style={IS} value={provider} onChange={e => setProvider(e.target.value)} placeholder="openai" disabled={creating} />
            )}
          </Field>
          <Field label="Model">
            <input style={IS} value={model} onChange={e => setModel(e.target.value)} placeholder="gpt-4o-mini" disabled={creating} />
          </Field>
        </div>
        <Field label="Bio" hint="Optional">
          <textarea style={{ ...IS, minHeight: 80, resize: 'vertical' }} value={bio} onChange={e => setBio(e.target.value)} placeholder="This agent specialises in…" disabled={creating} />
        </Field>
        <Field label="System prompt" hint="Optional override">
          <textarea style={{ ...IS, minHeight: 80, resize: 'vertical' }} value={system} onChange={e => setSystem(e.target.value)} placeholder="You are a helpful assistant that…" disabled={creating} />
        </Field>
        {error && <ErrorBox>{error}</ErrorBox>}
        <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 10, paddingTop: 4 }}>
          <button type="button" onClick={onClose} disabled={creating}
            style={{ padding: '10px 20px', fontSize: 13, background: 'transparent', border: `1px solid ${c.border}`, color: c.textMuted, cursor: 'pointer' }}>
            Cancel
          </button>
          <SubmitBtn disabled={creating || !name.trim()} loading={creating} accent={c.accent}>Create agent</SubmitBtn>
        </div>
      </form>
    </Modal>
  );
}

// ── New Swarm Modal ───────────────────────────────────────────────────────────
const STRATEGIES = {
  supervisor:    { label: 'Supervisor',   desc: 'Manager delegates subtasks to workers.' },
  dynamic:       { label: 'Dynamic',      desc: 'Workers self-organise by task requirements.' },
  'round-robin': { label: 'Round-robin',  desc: 'Tasks distributed evenly in rotation.' },
} as const;
type Strategy = keyof typeof STRATEGIES;

export function NewSwarmModal({ configuredProviders, onClose, onCreated, c }: {
  configuredProviders: ProviderResponse[];
  onClose: () => void;
  onCreated: (swarm: SwarmState) => void;
  c: Colors;
}) {
  const [strategy, setStrategy] = useState<Strategy>('supervisor');
  const [mgrName, setMgrName] = useState('');
  const [mgrModel, setMgrModel] = useState('');
  const [workers, setWorkers] = useState([{ name: '', model: '' }]);
  const [maxTurns, setMaxTurns] = useState('');
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const IS = inputStyle(c);

  const updateWorker = (i: number, key: 'name' | 'model', val: string) =>
    setWorkers(prev => prev.map((w, idx) => idx === i ? { ...w, [key]: val } : w));

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!mgrName.trim()) return;
    setCreating(true); setError(null);
    try {
      const fp = configuredProviders[0]?.id ?? 'openai';
      const ws: WorkerConfig[] = workers.filter(w => w.name.trim()).map(w => ({
        name: w.name.trim(), model: w.model.trim() || 'gpt-4o-mini', provider: fp,
      }));
      const req: SwarmCreateRequest = {
        strategy, maxTurns: maxTurns ? Number(maxTurns) : undefined,
        manager: { name: mgrName.trim(), model: mgrModel.trim() || 'gpt-4o-mini', provider: fp },
        workers: ws.length ? ws : [{ name: `${mgrName.trim()}-worker`, model: 'gpt-4o-mini', provider: fp }],
      };
      onCreated(await swarms.create(req));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally { setCreating(false); }
  }

  return (
    <Modal onClose={onClose} c={c}>
      <ModalHeader label="New swarm" title="Create a swarm" sub="Spin up a multi-agent coordination swarm." onClose={onClose} c={c} labelColor={c.success} />
      <form onSubmit={handleSubmit} style={{ padding: '24px 28px', display: 'flex', flexDirection: 'column', gap: 16 }}>
        <Field label="Strategy">
          <div style={{ display: 'grid', gridTemplateColumns: 'repeat(3, 1fr)', gap: 8 }}>
            {(Object.keys(STRATEGIES) as Strategy[]).map(k => (
              <button key={k} type="button" onClick={() => setStrategy(k)}
                style={{ padding: '10px 8px', fontSize: 12, textAlign: 'left', cursor: 'pointer', transition: 'all 0.1s',
                  background: strategy === k ? c.accentSoft : 'transparent',
                  border: `1px solid ${strategy === k ? c.accent : c.border}`,
                  color: strategy === k ? c.accent : c.textMuted }}>
                {STRATEGIES[k].label}
              </button>
            ))}
          </div>
          <div style={{ fontSize: 11, color: c.textMuted, marginTop: 4 }}>{STRATEGIES[strategy].desc}</div>
        </Field>
        <Field label="Manager">
          <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 10 }}>
            <input autoFocus style={IS} value={mgrName} onChange={e => setMgrName(e.target.value)} placeholder="Manager name *" disabled={creating} />
            <input style={IS} value={mgrModel} onChange={e => setMgrModel(e.target.value)} placeholder="gpt-4o-mini" disabled={creating} />
          </div>
        </Field>
        <Field label="Workers">
          <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
            {workers.map((w, i) => (
              <div key={i} style={{ display: 'flex', gap: 8 }}>
                <input style={{ ...IS, flex: 1 }} value={w.name} onChange={e => updateWorker(i, 'name', e.target.value)} placeholder={`Worker ${i + 1} name`} disabled={creating} />
                <input style={{ ...IS, width: 140 }} value={w.model} onChange={e => updateWorker(i, 'model', e.target.value)} placeholder="gpt-4o-mini" disabled={creating} />
                {workers.length > 1 && (
                  <button type="button" onClick={() => setWorkers(p => p.filter((_, idx) => idx !== i))} disabled={creating}
                    style={{ padding: '0 10px', background: 'transparent', border: `1px solid ${c.border}`, color: c.danger, cursor: 'pointer' }}>✕</button>
                )}
              </div>
            ))}
            <button type="button" onClick={() => setWorkers(p => [...p, { name: '', model: '' }])} disabled={creating}
              style={{ padding: '8px', fontSize: 12, background: 'transparent', border: `1px dashed ${c.border}`, color: c.textMuted, cursor: 'pointer' }}>
              + Add worker
            </button>
          </div>
        </Field>
        <Field label="Max turns" hint="Optional">
          <input style={IS} type="number" min="1" value={maxTurns} onChange={e => setMaxTurns(e.target.value)} placeholder="e.g. 10" disabled={creating} />
        </Field>
        {error && <ErrorBox>{error}</ErrorBox>}
        <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 10, paddingTop: 4 }}>
          <button type="button" onClick={onClose} disabled={creating}
            style={{ padding: '10px 20px', fontSize: 13, background: 'transparent', border: `1px solid ${c.border}`, color: c.textMuted, cursor: 'pointer' }}>
            Cancel
          </button>
          <SubmitBtn disabled={creating || !mgrName.trim()} loading={creating} accent={c.success}>Create swarm</SubmitBtn>
        </div>
      </form>
    </Modal>
  );
}

// ── Run Modal ─────────────────────────────────────────────────────────────────
export function RunModal({ kind, label, onClose, onRun, c }: {
  kind: 'agent' | 'swarm'; label: string; onClose: () => void;
  onRun: (task: string) => Promise<{ status: string; durationMs?: number; output: string | null; error: string | null }>;
  c: Colors;
}) {
  const [task, setTask] = useState('');
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<{ status: string; durationMs?: number; output: string | null; error: string | null } | null>(null);
  const IS = inputStyle(c);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!task.trim()) return;
    setRunning(true); setError(null);
    try { setResult(await onRun(task.trim())); setTask(''); }
    catch (err) { setError(err instanceof Error ? err.message : String(err)); }
    finally { setRunning(false); }
  }

  const accent = kind === 'agent' ? '#a78bfa' : '#38bdf8';
  return (
    <Modal onClose={onClose} c={c}>
      <ModalHeader label={`Run ${kind}`} title={label} sub="Send a task and wait for the daemon response." onClose={onClose} c={c} labelColor={accent} />
      <form onSubmit={handleSubmit} style={{ padding: '24px 28px', display: 'flex', flexDirection: 'column', gap: 16 }}>
        <textarea autoFocus style={{ ...IS, minHeight: 120, resize: 'vertical' }} value={task} onChange={e => setTask(e.target.value)}
          placeholder={kind === 'agent' ? 'Ask the agent to summarise, plan, research…' : 'Describe the coordination task for the swarm…'}
          disabled={running} />
        {error && <ErrorBox>{error}</ErrorBox>}
        {result && (
          <div style={{ padding: '14px 16px', border: `1px solid ${result.status === 'success' ? c.success + '40' : c.danger + '40'}`,
            background: result.status === 'success' ? 'rgba(34,197,94,0.06)' : 'rgba(248,113,113,0.06)' }}>
            <div style={{ display: 'flex', justifyContent: 'space-between', marginBottom: 8 }}>
              <span style={{ fontSize: 12, fontWeight: 600 }}>Result</span>
              <span style={{ fontSize: 9, fontFamily: MONO, letterSpacing: 1, textTransform: 'uppercase',
                color: result.status === 'success' ? c.success : c.danger }}>{result.status}</span>
            </div>
            {result.durationMs !== undefined && (
              <div style={{ fontSize: 11, color: c.textMuted, fontFamily: MONO, marginBottom: 8 }}>{(result.durationMs / 1000).toFixed(2)}s</div>
            )}
            {result.output && (
              <pre style={{ margin: 0, maxHeight: 200, overflowY: 'auto', fontSize: 11, fontFamily: MONO, lineHeight: 1.6, color: c.textSecondary, whiteSpace: 'pre-wrap', padding: '10px 12px', background: c.subtle, border: `1px solid ${c.border}` }}>{result.output}</pre>
            )}
            {result.error && <div style={{ fontSize: 12, color: c.danger, marginTop: 6 }}>{result.error}</div>}
          </div>
        )}
        <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 10, paddingTop: 4 }}>
          <button type="button" onClick={onClose}
            style={{ padding: '10px 20px', fontSize: 13, background: 'transparent', border: `1px solid ${c.border}`, color: c.textMuted, cursor: 'pointer' }}>
            Close
          </button>
          <SubmitBtn disabled={running || !task.trim()} loading={running} accent={accent}>Run</SubmitBtn>
        </div>
      </form>
    </Modal>
  );
}
