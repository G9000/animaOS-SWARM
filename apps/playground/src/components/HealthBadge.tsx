import { useEffect, useState } from 'react';
import { health, type HealthResponse } from '../lib/api';

export function HealthBadge() {
  const [h, setH] = useState<HealthResponse | null>(null);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    const tick = async () => {
      try {
        const result = await health.get();
        if (!cancelled) {
          setH(result);
          setErr(null);
        }
      } catch (e) {
        if (!cancelled) setErr(e instanceof Error ? e.message : String(e));
      }
    };
    tick();
    const id = setInterval(tick, 8000);
    return () => {
      cancelled = true;
      clearInterval(id);
    };
  }, []);

  if (err) {
    return (
      <span className="inline-flex items-center gap-1.5 px-2.5 py-1 text-xs rounded-full bg-[var(--err)]/10 text-[var(--err)] border border-[var(--err)]/30">
        <span className="w-1.5 h-1.5 rounded-full bg-[var(--err)]" />
        Offline
      </span>
    );
  }
  if (!h) {
    return (
      <span className="inline-flex items-center gap-1.5 px-2.5 py-1 text-xs rounded-full bg-[var(--surface-2)] text-[var(--muted)] border border-[var(--border)]">
        <span className="w-1.5 h-1.5 rounded-full bg-[var(--muted)]" />
        Connecting…
      </span>
    );
  }
  const uptime =
    h.uptime_secs !== undefined ? `${Math.round(h.uptime_secs / 60)}m` : '';
  return (
    <span className="inline-flex items-center gap-1.5 px-2.5 py-1 text-xs rounded-full bg-[var(--ok)]/10 text-[var(--ok)] border border-[var(--ok)]/30">
      <span
        className="w-1.5 h-1.5 rounded-full bg-[var(--ok)]"
        style={{ animation: 'pulse-dot 2.4s ease-in-out infinite' }}
      />
      <span className="capitalize">{h.status}</span>
      {h.version && <span className="text-[var(--muted)]">· v{h.version}</span>}
      {uptime && <span className="text-[var(--muted)]">· {uptime}</span>}
    </span>
  );
}
