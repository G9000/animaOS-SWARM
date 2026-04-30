import { useState } from 'react';
import { agents, type AgentConfig, type Provider } from '../lib/api';
import { ProviderSelect } from './ProviderSelect';

interface Props {
  onCreated: (id: string) => void;
  onCancel: () => void;
  providers: Provider[] | null;
}

export function CreateAgent({ onCreated, onCancel, providers }: Props) {
  const [name, setName] = useState('');
  const [model, setModel] = useState('claude-sonnet-4-6');
  const [provider, setProvider] = useState('');
  const [system, setSystem] = useState('');
  const [bio, setBio] = useState('');
  const [temperature, setTemperature] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setErr(null);
    setSubmitting(true);
    try {
      const cfg: AgentConfig = {
        name: name.trim(),
        model: model.trim(),
        provider: provider.trim() || undefined,
        system: system.trim() || undefined,
        bio: bio.trim() || undefined,
      };
      if (temperature.trim()) {
        cfg.settings = { temperature: parseFloat(temperature) };
      }
      const created = await agents.create(cfg);
      onCreated(created.state.id);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <form
      onSubmit={submit}
      className="max-w-xl mx-auto px-6 py-8 flex flex-col gap-5"
    >
      <FormHeader
        title="New agent"
        subtitle="Create a single agent and chat with it."
        onCancel={onCancel}
      />

      <Row>
        <Field label="Name" required>
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            required
            placeholder="research-bot"
            autoFocus
          />
        </Field>
        <Field label="Model" required>
          <input
            value={model}
            onChange={(e) => setModel(e.target.value)}
            required
          />
        </Field>
      </Row>

      <Row>
        <Field label="Provider">
          <ProviderSelect
            providers={providers}
            value={provider}
            onChange={setProvider}
          />
        </Field>
        <Field label="Temperature">
          <input
            type="number"
            step="0.1"
            min="0"
            max="2"
            value={temperature}
            onChange={(e) => setTemperature(e.target.value)}
            placeholder="0.7"
          />
        </Field>
      </Row>

      <Field label="System prompt">
        <textarea
          value={system}
          onChange={(e) => setSystem(e.target.value)}
          rows={4}
          placeholder="You are a helpful assistant…"
        />
      </Field>

      <Field label="Bio" hint="Optional persona description.">
        <textarea
          value={bio}
          onChange={(e) => setBio(e.target.value)}
          rows={2}
        />
      </Field>

      {err && <FormError message={err} />}

      <div className="flex gap-2 mt-2">
        <button
          type="submit"
          disabled={submitting || !name.trim() || !model.trim()}
          className="px-4 py-2 text-sm font-medium rounded-md bg-[var(--accent)] text-[var(--accent-fg)] hover:bg-[var(--accent-hover)] disabled:bg-[var(--surface-3)] disabled:text-[var(--muted-2)] disabled:cursor-not-allowed transition-colors"
        >
          {submitting ? 'Creating…' : 'Create agent'}
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

export function FormHeader({
  title,
  subtitle,
  onCancel,
}: {
  title: string;
  subtitle: string;
  onCancel: () => void;
}) {
  return (
    <div className="flex items-start justify-between border-b border-[var(--border)] pb-4">
      <div>
        <h2 className="text-lg font-semibold text-[var(--text)] m-0">
          {title}
        </h2>
        <p className="text-sm text-[var(--muted)] mt-1 m-0">{subtitle}</p>
      </div>
      <button
        type="button"
        onClick={onCancel}
        className="text-sm text-[var(--muted)] hover:text-[var(--text)] px-2 py-1 rounded hover:bg-[var(--hover)] transition-colors"
      >
        Close
      </button>
    </div>
  );
}

export function FormError({ message }: { message: string }) {
  return (
    <div className="text-sm text-[var(--err)] bg-[var(--err)]/10 border border-[var(--err)]/30 rounded-md px-3 py-2">
      {message}
    </div>
  );
}

export function Row({ children }: { children: React.ReactNode }) {
  return <div className="grid grid-cols-2 gap-4">{children}</div>;
}

export function Field({
  label,
  required,
  hint,
  children,
}: {
  label: string;
  required?: boolean;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <label className="flex flex-col gap-1.5">
      <span className="text-sm text-[var(--text-2)] font-medium">
        {label}
        {required && <span className="text-[var(--err)] ml-0.5">*</span>}
      </span>
      {children}
      {hint && (
        <span className="text-xs text-[var(--muted-2)]">{hint}</span>
      )}
    </label>
  );
}
