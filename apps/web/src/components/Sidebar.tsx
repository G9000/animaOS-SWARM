import type { AgentDetail } from '../lib/types';
import { BoltIcon, GearIcon, PulseIcon, SparkIcon } from './icons';
import { formatTokens } from './ui-bits';

function LogoMark({ compact }: { compact?: boolean }) {
  return (
    <div className={`relative flex items-center justify-center ${compact ? 'h-8 w-8' : 'h-7 w-7'}`}>
      <div className="absolute inset-0 rounded-[9px] bg-sky-500 opacity-90 shadow-lg shadow-sky-500/25" />
      <div className="absolute inset-[3px] rounded-[6px] bg-abyss/80" />
      <div className="relative h-2 w-2 rounded-[3px] bg-sky-300" />
    </div>
  );
}

const STATUS_DOT: Record<AgentDetail['status'], string> = {
  Idle: 'bg-mint',
  Running: 'bg-sky-400',
  Completed: 'bg-violet-400',
  Failed: 'bg-red-400',
  Terminated: 'bg-zinc-500',
};

function NavItem({
  icon,
  label,
  active,
  badge,
  collapsed,
  onClick,
}: {
  icon: React.ReactNode;
  label: string;
  active?: boolean;
  badge?: number;
  collapsed: boolean;
  onClick?: () => void;
}) {
  return (
    <button
      onClick={onClick}
      title={collapsed ? label : undefined}
      className={`group relative flex w-full cursor-pointer items-center gap-3 rounded-xl px-3 py-2.5 text-[13px] font-medium transition-all duration-150 ${
        active
          ? 'bg-sky-500/10 text-ink'
          : 'text-ink-3 hover:bg-white/[0.04] hover:text-ink-2'
      } ${collapsed ? 'justify-center px-0' : ''}`}
    >
      {active && (
        <span className="absolute left-0 top-1/2 h-5 w-[3px] -translate-y-1/2 rounded-full bg-sky-400" />
      )}
      <span className={`shrink-0 ${active ? 'text-sky-400' : 'text-ink-3 group-hover:text-ink-2'}`}>
        {icon}
      </span>
      {!collapsed && <span className="flex-1 truncate text-left">{label}</span>}
      {!collapsed && badge !== undefined && badge > 0 && (
        <span className="rounded-full border border-sky-400/30 bg-sky-400/10 px-1.5 py-px font-mono text-[10px] text-sky-300">
          {badge}
        </span>
      )}
      {collapsed && badge !== undefined && badge > 0 && (
        <span className="absolute right-1.5 top-1.5 h-1.5 w-1.5 rounded-full bg-sky-400" />
      )}
    </button>
  );
}

export function Sidebar({
  agent,
  online,
  collapsed,
  activeView,
  checkinCount,
  onNavigate,
  onToggleCollapse,
  onOpenSettings,
}: {
  agent: AgentDetail | null;
  online: boolean | null;
  collapsed: boolean;
  activeView: 'chat' | 'checkins';
  checkinCount: number;
  onNavigate: (view: 'chat' | 'checkins') => void;
  onToggleCollapse: () => void;
  onOpenSettings: () => void;
}) {
  const totalTokens = agent?.token_usage?.total_tokens ?? 0;

  return (
    <aside
      className={`relative z-20 flex h-full shrink-0 flex-col border-r border-line bg-panel/50 backdrop-blur-xl transition-[width] duration-300 ease-[cubic-bezier(0.16,1,0.3,1)] ${
        collapsed ? 'w-[68px]' : 'w-64'
      }`}
    >
      {/* Brand */}
      <div className={`flex h-16 items-center border-b border-line ${collapsed ? 'justify-center px-0' : 'gap-3 px-4'}`}>
        <LogoMark compact={collapsed} />
        {!collapsed && (
          <div className="flex min-w-0 flex-1 items-baseline gap-1.5">
            <span className="font-display text-sm font-bold uppercase tracking-[0.22em] text-ink">
              anima<span className="text-gradient">OS</span>
            </span>
          </div>
        )}
        {!collapsed && (
          <button
            onClick={onToggleCollapse}
            className="flex h-6 w-6 cursor-pointer items-center justify-center rounded-md text-ink-3 transition hover:bg-white/[0.05] hover:text-ink"
            aria-label="Collapse sidebar"
          >
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <path d="m15 18-6-6 6-6" />
            </svg>
          </button>
        )}
      </div>

      {/* Nav */}
      <nav className={`flex flex-col gap-1 pt-4 ${collapsed ? 'items-center px-2' : 'px-3'}`}>
        {!collapsed && (
          <span className="mb-1 px-3 font-mono text-[10px] font-medium uppercase tracking-[0.18em] text-ink-3/70">
            Menu
          </span>
        )}
        <NavItem
          icon={<SparkIcon size={16} />}
          label="Chat"
          active={activeView === 'chat'}
          collapsed={collapsed}
          onClick={() => onNavigate('chat')}
        />
        <NavItem
          icon={<PulseIcon size={16} />}
          label="Proactive"
          active={activeView === 'checkins'}
          badge={checkinCount}
          collapsed={collapsed}
          onClick={() => onNavigate('checkins')}
        />
        <NavItem icon={<GearIcon size={16} />} label="Settings" collapsed={collapsed} onClick={onOpenSettings} />
      </nav>

      {/* Agent card */}
      <div className={`mt-6 ${collapsed ? 'flex justify-center px-2' : 'px-3'}`}>
        {agent ? (
          collapsed ? (
            <button
              onClick={onOpenSettings}
              title={agent.name}
              className="relative flex h-10 w-10 cursor-pointer items-center justify-center rounded-xl bg-sky-500 font-display text-sm font-bold text-white shadow-lg shadow-sky-500/20 transition hover:scale-105"
            >
              {agent.name.charAt(0).toUpperCase()}
              <span className={`absolute -bottom-0.5 -right-0.5 h-2.5 w-2.5 rounded-full border-2 border-panel ${STATUS_DOT[agent.status]}`} />
            </button>
          ) : (
            <div className="glass animate-fade-in rounded-2xl p-3.5">
              <span className="mb-2.5 block font-mono text-[10px] font-medium uppercase tracking-[0.18em] text-ink-3/70">
                Your agent
              </span>
              <div className="flex items-center gap-2.5">
                <div className="relative flex h-9 w-9 shrink-0 items-center justify-center rounded-xl bg-sky-500 font-display text-sm font-bold text-white">
                  {agent.name.charAt(0).toUpperCase()}
                  <span className={`absolute -bottom-0.5 -right-0.5 h-2.5 w-2.5 rounded-full border-2 border-panel ${STATUS_DOT[agent.status]}`} />
                </div>
                <div className="min-w-0">
                  <div className="truncate text-[13px] font-semibold text-ink">{agent.name}</div>
                  <div className="truncate font-mono text-[10px] text-ink-3">{agent.model}</div>
                </div>
              </div>
              <div className="mt-3 grid grid-cols-2 gap-1.5">
                <div className="rounded-lg border border-line bg-white/[0.02] px-2 py-1.5">
                  <div className="font-mono text-[9px] uppercase tracking-wider text-ink-3/70">tokens</div>
                  <div className="font-mono text-xs font-medium text-ink-2">{formatTokens(totalTokens)}</div>
                </div>
                <div className="rounded-lg border border-line bg-white/[0.02] px-2 py-1.5">
                  <div className="font-mono text-[9px] uppercase tracking-wider text-ink-3/70">messages</div>
                  <div className="font-mono text-xs font-medium text-ink-2">{agent.messages.length}</div>
                </div>
              </div>
            </div>
          )
        ) : (
          !collapsed && (
            <div className="rounded-2xl border border-dashed border-line px-3.5 py-4 text-center">
              <BoltIcon size={16} className="mx-auto mb-1.5 text-ink-3" />
              <p className="text-[11px] leading-relaxed text-ink-3">no agent yet — create one to begin</p>
            </div>
          )
        )}
      </div>

      {/* Footer */}
      <div className={`mt-auto border-t border-line ${collapsed ? 'flex flex-col items-center gap-2 px-2 py-3' : 'px-4 py-3'}`}>
        {collapsed ? (
          <>
            <span
              title={online === false ? 'daemon offline' : 'daemon connected'}
              className={`h-2 w-2 rounded-full ${online === false ? 'bg-red-400' : 'bg-mint'}`}
            />
            <button
              onClick={onToggleCollapse}
              className="flex h-6 w-6 cursor-pointer items-center justify-center rounded-md text-ink-3 transition hover:bg-white/[0.05] hover:text-ink"
              aria-label="Expand sidebar"
            >
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <path d="m9 18 6-6-6-6" />
              </svg>
            </button>
          </>
        ) : (
          <div className="flex items-center justify-between">
            <span className="flex items-center gap-2 font-mono text-[10px] text-ink-3">
              <span className={`h-1.5 w-1.5 rounded-full ${online === false ? 'bg-red-400' : 'bg-mint'}`} />
              {online === false ? 'daemon offline' : 'daemon connected'}
            </span>
            <span className="font-mono text-[10px] text-ink-3/60">v0.1</span>
          </div>
        )}
      </div>
    </aside>
  );
}
