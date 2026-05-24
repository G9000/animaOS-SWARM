import React, { CSSProperties } from 'react';
import { Colors, MONO, sparkPath, avatarUrl, statusColor, genSeries } from './design';

// ── Sparkline ─────────────────────────────────────────────────────────────────
export function Sparkline({
  series, width = 120, height = 32, color = '#38bdf8', fill = true,
}: { series: number[]; width?: number; height?: number; color?: string; fill?: boolean }) {
  const path = sparkPath(series, width, height, 2);
  const area = `${path} L ${width - 2} ${height} L 2 ${height} Z`;
  const last = series[series.length - 1];
  const cy = 2 + (height - 4) * (1 - last);
  return (
    <svg width={width} height={height} style={{ display: 'block', overflow: 'visible' }}>
      {fill && <path d={area} fill={color} fillOpacity="0.14" />}
      <path d={path} fill="none" stroke={color} strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
      <circle cx={width - 2} cy={cy} r="2.5" fill={color} />
    </svg>
  );
}

// ── Animated activity chart (two series) ─────────────────────────────────────
export function ActivityChart({
  series, series2, c, width = 600, height = 120,
}: { series: number[]; series2: number[]; c: Colors; width?: number; height?: number }) {
  const p1 = sparkPath(series, width, height, 4);
  const p2 = sparkPath(series2, width, height, 4);
  const fill1 = `${p1} L ${width - 4} ${height} L 4 ${height} Z`;
  return (
    <svg width="100%" height="100%" viewBox={`0 0 ${width} ${height}`} preserveAspectRatio="none" style={{ display: 'block' }}>
      <defs>
        <linearGradient id="actGrad" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor={c.accent} stopOpacity="0.22" />
          <stop offset="100%" stopColor={c.accent} stopOpacity="0" />
        </linearGradient>
      </defs>
      {[0.25, 0.5, 0.75].map(p => (
        <line key={p} x1="4" y1={height * p} x2={width - 4} y2={height * p}
          stroke={c.border} strokeWidth="0.5" />
      ))}
      <path d={fill1} fill="url(#actGrad)" />
      <path d={p1} fill="none" stroke={c.accent} strokeWidth="1.5" />
      <path d={p2} fill="none" stroke={c.textMuted} strokeWidth="0.8" strokeDasharray="3 3" opacity="0.5" />
      <circle cx={width - 4} cy={height - height * series[series.length - 1]} r="3" fill={c.accent} />
    </svg>
  );
}

// ── Mini chart card ───────────────────────────────────────────────────────────
export function MiniChart({
  title, series, c, accent, unit, mult,
}: { title: string; series: number[]; c: Colors; accent: string; unit: string; mult: number }) {
  const w = 400, h = 60;
  const path = sparkPath(series, w, h, 4);
  const fill = `${path} L ${w - 4} ${h} L 4 ${h} Z`;
  const cur = (series[series.length - 1] * mult).toFixed(0);
  const gid = `mg-${accent.replace('#', '')}`;
  return (
    <div style={{ border: `1px solid ${c.border}`, padding: '14px 16px', background: c.elevated }}>
      <div style={{ display: 'flex', justifyContent: 'space-between', marginBottom: 10 }}>
        <div style={{ fontSize: 10, color: c.textMuted, fontFamily: MONO, letterSpacing: 0.8, textTransform: 'uppercase' }}>{title}</div>
        <div style={{ fontSize: 18, fontWeight: 700, fontFamily: MONO }}>{cur}{unit}</div>
      </div>
      <svg width="100%" height="50" viewBox={`0 0 ${w} ${h}`} preserveAspectRatio="none" style={{ display: 'block' }}>
        <defs>
          <linearGradient id={gid} x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor={accent} stopOpacity="0.2" />
            <stop offset="100%" stopColor={accent} stopOpacity="0" />
          </linearGradient>
        </defs>
        <path d={fill} fill={`url(#${gid})`} />
        <path d={path} fill="none" stroke={accent} strokeWidth="1.5" />
        <circle cx={w - 4} cy={h - h * series[series.length - 1]} r="3" fill={accent} />
      </svg>
    </div>
  );
}

// ── Status badge ──────────────────────────────────────────────────────────────
export function StatusBadge({ status, dark, c, size = 'sm' }: {
  status: string; dark: boolean; c: Colors; size?: 'sm' | 'md';
}) {
  const color = statusColor(status, dark);
  const bg =
    status === 'running' ? (dark ? 'rgba(34,197,94,0.12)' : 'rgba(22,163,74,0.08)') :
    status === 'failed'  ? (dark ? 'rgba(248,113,113,0.12)' : 'rgba(220,38,38,0.08)') :
    (dark ? 'rgba(148,163,184,0.1)' : 'rgba(107,114,128,0.08)');
  return (
    <span style={{
      fontSize: size === 'md' ? 11 : 9,
      padding: size === 'md' ? '4px 10px' : '2px 8px',
      letterSpacing: 0.8, background: bg, color,
      fontFamily: MONO, textTransform: 'uppercase',
      border: `1px solid ${color}40`,
    }}>{status}</span>
  );
}

// ── Agent avatar with status dot ──────────────────────────────────────────────
export function AgentAvatar({ name, size = 36, status, dark, c }: {
  name: string; size?: number; status?: string; dark?: boolean; c: Colors;
}) {
  const dotColor = status && dark !== undefined ? statusColor(status, dark) : undefined;
  return (
    <div style={{ position: 'relative', width: size, height: size, flexShrink: 0 }}>
      <img src={avatarUrl(name)} alt={name}
        style={{
          width: size, height: size, objectFit: 'cover', display: 'block',
          border: `1px solid ${c.border}`, background: c.subtle,
          opacity: status === 'terminated' ? 0.4 : 1,
        }}
      />
      {dotColor && (
        <span style={{
          position: 'absolute', bottom: -1, right: -1,
          width: size > 28 ? 9 : 7, height: size > 28 ? 9 : 7,
          background: dotColor, border: `2px solid ${c.bg}`,
        }} />
      )}
    </div>
  );
}

// ── Filter pills ──────────────────────────────────────────────────────────────
export function FilterPills({ options, value, onChange, c, dark }: {
  options: string[]; value: string; onChange: (v: string) => void; c: Colors; dark: boolean;
}) {
  return (
    <div style={{ display: 'flex', border: `1px solid ${c.border}` }}>
      {options.map((o, i) => (
        <button key={o} onClick={() => onChange(o)} style={{
          padding: '7px 12px', fontSize: 11, fontFamily: MONO, cursor: 'pointer',
          background: value === o ? c.accent : 'transparent',
          color: value === o ? (dark ? '#0f1115' : '#fff') : c.textMuted,
          border: 'none', borderRight: i < options.length - 1 ? `1px solid ${c.border}` : 'none',
          letterSpacing: 0.5,
        }}>{o}</button>
      ))}
    </div>
  );
}

// ── Search input ──────────────────────────────────────────────────────────────
export function SearchInput({ value, onChange, placeholder, c, style }: {
  value: string; onChange: (v: string) => void; placeholder?: string; c: Colors; style?: CSSProperties;
}) {
  return (
    <div style={{ position: 'relative', ...style }}>
      <span style={{ position: 'absolute', left: 10, top: '50%', transform: 'translateY(-50%)', color: c.textMuted, fontSize: 14 }}>⌕</span>
      <input value={value} onChange={e => onChange(e.target.value)} placeholder={placeholder ?? 'Search…'}
        style={{
          width: '100%', padding: '8px 10px 8px 30px',
          background: c.elevated, color: c.textPrimary,
          border: `1px solid ${c.border}`, outline: 'none',
          fontSize: 12, fontFamily: 'inherit',
        }}
      />
    </div>
  );
}

// ── Stat strip (horizontal row of metrics) ────────────────────────────────────
export function StatStrip({ stats, c }: {
  stats: { label: string; value: string | number; sub?: string; highlight?: string }[];
  c: Colors;
}) {
  return (
    <div style={{ display: 'grid', gridTemplateColumns: `repeat(${stats.length}, 1fr)`, border: `1px solid ${c.border}` }}>
      {stats.map(({ label, value, sub, highlight }, i) => (
        <div key={label} style={{
          padding: '16px 20px',
          borderRight: i < stats.length - 1 ? `1px solid ${c.border}` : 'none',
        }}>
          <div style={{ fontSize: 9, color: c.textMuted, fontFamily: MONO, letterSpacing: 1.2, textTransform: 'uppercase', marginBottom: 6 }}>{label}</div>
          <div style={{ fontSize: 26, fontWeight: 800, letterSpacing: -0.8, color: highlight ?? c.textPrimary }}>{value}</div>
          {sub && <div style={{ fontSize: 10, color: c.textMuted, marginTop: 3, fontFamily: MONO }}>{sub}</div>}
        </div>
      ))}
    </div>
  );
}

// ── Panel header ──────────────────────────────────────────────────────────────
export function PanelHeader({ title, sub, actions, c }: {
  title: string; sub?: string; actions?: React.ReactNode; c: Colors;
}) {
  return (
    <div style={{ padding: '12px 16px', borderBottom: `1px solid ${c.border}`,
      display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
      <div>
        <div style={{ fontSize: 12, fontWeight: 600 }}>{title}</div>
        {sub && <div style={{ fontSize: 10, color: c.textMuted, fontFamily: MONO, marginTop: 1 }}>{sub}</div>}
      </div>
      {actions}
    </div>
  );
}

// ── Topology mini-map ─────────────────────────────────────────────────────────
export function MiniTopology({ agents, c, dark, tick }: {
  agents: { name: string; status: string }[];
  c: Colors; dark: boolean; tick: number;
}) {
  const W = 320, H = 180, cx = W / 2, cy = H / 2;
  const center = agents[0];
  const ring = agents.slice(1, 12);
  const nodes = ring.map((a, i) => {
    const ang = (i / ring.length) * Math.PI * 2 - Math.PI / 2;
    return { ...a, x: cx + Math.cos(ang) * 68, y: cy + Math.sin(ang) * 68 };
  });
  return (
    <svg width="100%" viewBox={`0 0 ${W} ${H}`} style={{ display: 'block' }}>
      <defs>
        <radialGradient id="topoGlow" cx="50%" cy="50%" r="50%">
          <stop offset="0%" stopColor={c.accent} stopOpacity="0.1" />
          <stop offset="100%" stopColor={c.accent} stopOpacity="0" />
        </radialGradient>
      </defs>
      <circle cx={cx} cy={cy} r={85} fill="url(#topoGlow)" />
      <circle cx={cx} cy={cy} r={68} fill="none" stroke={c.border} strokeWidth="0.5" strokeDasharray="2 4" />
      {nodes.map((n, i) => {
        const active = n.status === 'running';
        const phase = ((tick * 0.25 + i * 0.15) % 1);
        return (
          <g key={n.name}>
            <line x1={cx} y1={cy} x2={n.x} y2={n.y} stroke={active ? c.accent : c.border} strokeWidth="0.6" opacity={active ? 0.35 : 0.2} />
            {active && <circle cx={cx + (n.x - cx) * phase} cy={cy + (n.y - cy) * phase} r="1.5" fill={c.success} opacity={1 - phase * 0.6} />}
          </g>
        );
      })}
      {/* center node */}
      {center && (
        <g>
          <image href={avatarUrl(center.name)} x={cx - 16} y={cy - 16} width={32} height={32} />
          <rect x={cx - 16} y={cy - 16} width={32} height={32} fill="none" stroke={c.accent} strokeWidth="1.5" />
          <rect x={cx - 18} y={cy - 18} width={36} height={36} fill="none" stroke={c.accent} strokeWidth="0.5" opacity="0.4" />
        </g>
      )}
      {/* ring nodes */}
      {nodes.map(n => {
        const sc = statusColor(n.status, dark);
        return (
          <g key={n.name + '-node'}>
            <image href={avatarUrl(n.name)} x={n.x - 8} y={n.y - 8} width={16} height={16} />
            <rect x={n.x - 8} y={n.y - 8} width={16} height={16} fill="none" stroke={sc} strokeWidth="0.8" />
          </g>
        );
      })}
    </svg>
  );
}
