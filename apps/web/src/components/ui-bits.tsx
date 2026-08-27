import type { ReactNode } from 'react';

export const labelCls =
  'mb-1.5 block font-mono text-[11px] font-medium uppercase tracking-[0.14em] text-ink-3';

export const primaryBtnCls =
  'inline-flex items-center justify-center gap-2 rounded-xl bg-accent px-4 py-2 text-sm font-semibold text-abyss shadow-lg shadow-accent/20 transition hover:bg-accent/90 hover:shadow-accent/30 active:scale-[0.98] disabled:cursor-not-allowed disabled:opacity-40 disabled:shadow-none disabled:active:scale-100';

export const ghostBtnCls =
  'inline-flex items-center gap-1.5 rounded-lg border border-line bg-white/[0.02] px-3 py-1.5 text-xs font-medium text-ink-2 transition hover:border-line-strong hover:bg-white/[0.05] hover:text-ink';

export function Switch({
  checked,
  onChange,
  label,
}: {
  checked: boolean;
  onChange: (next: boolean) => void;
  label?: string;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      onClick={() => onChange(!checked)}
      className={`relative h-5.5 w-10 shrink-0 cursor-pointer rounded-full border transition-colors duration-200 ${
        checked
          ? 'border-accent/60 bg-accent/85'
          : 'border-line-strong bg-white/[0.05]'
      }`}
      style={{ height: 22 }}
    >
      <span
        className={`absolute top-1/2 h-4 w-4 -translate-y-1/2 rounded-full bg-white shadow transition-all duration-200 ${
          checked ? 'left-[calc(100%-1.25rem)]' : 'left-0.5'
        }`}
      />
    </button>
  );
}

export function SectionTitle({ children }: { children: ReactNode }) {
  return (
    <h4 className="font-mono text-[11px] font-semibold uppercase tracking-[0.16em] text-ink-3">
      {children}
    </h4>
  );
}

export function ErrorBanner({
  message,
  onDismiss,
  icon,
}: {
  message: string;
  onDismiss?: () => void;
  icon?: ReactNode;
}) {
  return (
    <div className="animate-fade-in flex items-start gap-2.5 rounded-xl border border-danger/25 bg-danger/[0.08] px-3.5 py-2.5 text-xs leading-relaxed text-danger">
      <span className="mt-0.5 shrink-0 text-danger">{icon}</span>
      <span className="flex-1 break-words">{message}</span>
      {onDismiss && (
        <button
          onClick={onDismiss}
          className="shrink-0 cursor-pointer text-danger transition hover:text-ink"
          aria-label="Dismiss"
        >
          ✕
        </button>
      )}
    </div>
  );
}

export function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return String(n);
}

export function formatTime(ms: number): string {
  return new Date(ms).toLocaleTimeString([], {
    hour: '2-digit',
    minute: '2-digit',
  });
}
