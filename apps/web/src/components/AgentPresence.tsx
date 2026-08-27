import { deriveAccessProfile } from '../lib/agent-access';
import type { DaemonConnection } from '../hooks/useDaemonBootstrap';
import type { AgentDetail } from '../lib/types';
import { AlertIcon, BoltIcon, GearIcon, ShieldIcon, SparkIcon } from './icons';
import { ghostBtnCls } from './ui-bits';

const AGENT_STATUS_LABEL: Record<AgentDetail['status'], string> = {
  Idle: 'Idle',
  Running: 'Running',
  Completed: 'Completed',
  Failed: 'Failed',
  Terminated: 'Terminated',
};

function titleCase(value: string) {
  return `${value.charAt(0).toUpperCase()}${value.slice(1)}`;
}

function StatusLabel({
  icon,
  iconLabel,
  children,
}: {
  icon: React.ReactNode;
  iconLabel: string;
  children: React.ReactNode;
}) {
  return (
    <span className="inline-flex items-center gap-1.5 rounded-full border border-line bg-white/[0.025] px-2.5 py-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-2">
      <span role="img" aria-label={iconLabel} className="text-sky-400">
        {icon}
      </span>
      {children}
    </span>
  );
}

export function AgentPresence({
  agent,
  connection,
  onOpenSettings,
}: {
  agent: AgentDetail;
  connection: Exclude<DaemonConnection, 'unknown'>;
  onOpenSettings: () => void;
}) {
  const accessProfile = deriveAccessProfile(agent.toolNames);
  const accessLabel = titleCase(accessProfile);
  const agentStatus = AGENT_STATUS_LABEL[agent.status];
  const online = connection === 'online';

  return (
    <header className="flex min-w-0 items-center justify-between gap-4 border-b border-line bg-panel/50 px-4 py-3 backdrop-blur-xl sm:px-6">
      <div className="flex min-w-0 items-center gap-3">
        <div
          className="relative flex h-11 w-11 shrink-0 items-center justify-center"
          aria-hidden
        >
          <span className="absolute inset-0 rounded-full border border-sky-400/20" />
          <span className="absolute inset-1 rounded-full bg-sky-500/15" />
          <span className="relative h-3 w-3 rounded-full bg-sky-300 shadow-[0_0_18px_rgba(125,211,252,0.8)]" />
        </div>
        <div className="min-w-0">
          <p className="font-mono text-[10px] uppercase tracking-[0.16em] text-ink-3">
            Welcome back
          </p>
          <div className="mt-0.5 flex min-w-0 items-baseline gap-2">
            <h1 className="truncate font-display text-base font-semibold tracking-tight text-ink">
              {agent.name}
            </h1>
            <span className="rounded-full border border-sky-400/30 bg-sky-400/10 px-2 py-0.5 font-mono text-[9px] uppercase tracking-wider text-sky-300">
              Main
            </span>
          </div>
          <div className="mt-2 flex flex-wrap gap-1.5">
            <StatusLabel
              icon={online ? <BoltIcon size={11} /> : <AlertIcon size={11} />}
              iconLabel={`Daemon ${online ? 'online' : 'offline'}`}
            >
              Daemon {online ? 'Online' : 'Offline'}
            </StatusLabel>
            <StatusLabel
              icon={<SparkIcon size={11} />}
              iconLabel={`Agent ${agentStatus.toLowerCase()}`}
            >
              Agent {agentStatus}
            </StatusLabel>
            <StatusLabel
              icon={<ShieldIcon size={11} />}
              iconLabel={`${accessLabel} access profile`}
            >
              Access {accessLabel}
            </StatusLabel>
          </div>
        </div>
      </div>
      <button
        type="button"
        onClick={onOpenSettings}
        className={`${ghostBtnCls} shrink-0`}
        aria-label="Settings"
        title={`Settings for ${agent.name}`}
      >
        <GearIcon size={13} />
        <span className="hidden sm:inline">Settings</span>
      </button>
    </header>
  );
}
