// ── Design tokens & shared helpers ───────────────────────────────────────────
// Drop this into apps/web/src/

export interface Colors {
  bg: string; elevated: string; subtle: string; sidebar: string;
  border: string; borderStrong: string;
  textPrimary: string; textSecondary: string; textMuted: string;
  accent: string; accentLight: string; accentSoft: string;
  success: string; danger: string; warn: string;
}

export const DARK: Colors = {
  bg: '#0f1115', elevated: '#181a20', subtle: '#111317', sidebar: '#13151a',
  border: 'rgba(148,163,184,0.18)', borderStrong: 'rgba(56,189,248,0.28)',
  textPrimary: '#f8fafc', textSecondary: '#cbd5e1', textMuted: '#94a3b8',
  accent: '#38bdf8', accentLight: 'rgba(56,189,248,0.07)', accentSoft: 'rgba(56,189,248,0.12)',
  success: '#22c55e', danger: '#f87171', warn: '#fbbf24',
};

export const LIGHT: Colors = {
  bg: '#f3f4f6', elevated: '#ffffff', subtle: '#f9fafb', sidebar: '#fafafa',
  border: '#e5e7eb', borderStrong: '#d1d5db',
  textPrimary: '#111827', textSecondary: '#4b5563', textMuted: '#9ca3af',
  accent: '#ea580c', accentLight: '#fff7ed', accentSoft: 'rgba(234,88,12,0.08)',
  success: '#16a34a', danger: '#dc2626', warn: '#b45309',
};

export const MONO = "'IBM Plex Mono', ui-monospace, monospace";
export const SANS = "'Inter', ui-sans-serif, sans-serif";

export const getColors = (dark: boolean): Colors => dark ? DARK : LIGHT;

export function fmtTokens(n: number): string {
  if (n >= 1_000_000) return (n / 1_000_000).toFixed(1) + 'M';
  if (n >= 1_000) return (n / 1_000).toFixed(0) + 'k';
  return String(n);
}

export function relativeTime(ms: number): string {
  const s = Math.floor((Date.now() - ms) / 1000);
  if (s < 60) return `${s}s ago`;
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  return `${Math.floor(s / 3600)}h ago`;
}

export const avatarUrl = (name: string) =>
  `https://api.dicebear.com/9.x/adventurer/svg?seed=${encodeURIComponent(name)}&backgroundType=gradientLinear&backgroundColor=b6e3f4,c0aede,d1d4f9,ffd5dc,ffdfbf`;

export function sparkPath(values: number[], w: number, h: number, pad = 2): string {
  const min = Math.min(...values), max = Math.max(...values);
  const range = max - min || 1;
  const step = (w - pad * 2) / (values.length - 1);
  return values
    .map((v, i) => {
      const x = pad + i * step;
      const y = pad + (h - pad * 2) * (1 - (v - min) / range);
      return `${i === 0 ? 'M' : 'L'}${x.toFixed(1)} ${y.toFixed(1)}`;
    })
    .join(' ');
}

export function genSeries(n: number, base: number, vol: number, seed: number): number[] {
  let s = seed;
  const rand = () => { s = (s * 9301 + 49297) % 233280; return s / 233280; };
  let v = base;
  return Array.from({ length: n }, () => {
    v = Math.max(0.05, Math.min(0.98, v + (rand() - 0.5) * vol));
    return v;
  });
}

export function tokenLoad(totalTokens: number, model: string): number {
  return Math.min(0.98, totalTokens / (model.includes('opus') ? 2_000_000 : 1_000_000));
}

export function statusColor(status: string, dark: boolean): string {
  const map: Record<string, [string, string]> = {
    running:    ['#22c55e', '#16a34a'],
    idle:       ['#94a3b8', '#64748b'],
    failed:     ['#f87171', '#dc2626'],
    terminated: ['#94a3b8', '#94a3b8'],
    completed:  ['#22c55e', '#16a34a'],
  };
  return (map[status] ?? ['#94a3b8', '#64748b'])[dark ? 0 : 1];
}

export const EVENT_TYPE_COLOR: Record<string, string> = {
  'task:completed': '#22c55e', 'task:failed': '#f87171',
  'task:started': '#38bdf8',  'agent:spawned': '#a78bfa',
  'agent:tokens': '#fbbf24',  'tool:after': '#38bdf8',
  'tool:before': '#94a3b8',   'swarm:message': '#94a3b8',
  'swarm:running': '#38bdf8', 'swarm:completed': '#22c55e',
  'agent:terminated': '#f87171',
};
