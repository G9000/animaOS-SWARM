import type { Provider } from '../lib/api';

interface Props {
  providers: Provider[] | null;
  value: string;
  onChange: (value: string) => void;
  id?: string;
}

export function ProviderSelect({ providers, value, onChange, id }: Props) {
  if (!providers) {
    return (
      <select disabled value="">
        <option>Loading providers…</option>
      </select>
    );
  }

  const selected = providers.find((p) => p.id === value);
  const note =
    selected && selected.requiresKey && !selected.configured
      ? `${selected.apiKeyEnvs[0] ?? 'API key'} is not set in the daemon environment — runs will fail.`
      : null;

  return (
    <div className="flex flex-col gap-1">
      <select id={id} value={value} onChange={(e) => onChange(e.target.value)}>
        <option value="">Default (deterministic)</option>
        {providers.map((p) => {
          const suffix = !p.requiresKey
            ? ''
            : p.configured
            ? ''
            : ' — no key';
          return (
            <option key={p.id} value={p.id}>
              {p.label}
              {suffix}
            </option>
          );
        })}
      </select>
      {note && (
        <span className="text-xs text-[var(--warn)]">{note}</span>
      )}
    </div>
  );
}
