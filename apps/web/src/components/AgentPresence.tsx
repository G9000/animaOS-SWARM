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
  iconClassName = 'text-ink-3',
  children,
}: {
  icon: React.ReactNode;
  iconLabel: string;
  iconClassName?: string;
  children: React.ReactNode;
}) {
  return (
    <span className="inline-flex items-center gap-1.5 rounded-full border border-line bg-white/[0.025] px-2.5 py-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-2">
      <span role="img" aria-label={iconLabel} className={iconClassName}>
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
    <header className="relative flex min-w-0 items-center justify-between gap-4 border-b border-line bg-abyss/65 px-4 py-2.5 backdrop-blur-2xl sm:px-6">
      <div className="flex min-w-0 items-center gap-3">
        <div
          className="relative flex h-11 w-11 shrink-0 items-center justify-center"
          aria-hidden
        >
          <span className="absolute inset-0 rounded-full border border-accent/20" />
          <span className="absolute inset-1.5 rounded-full border border-accent/25 bg-accent/[0.08]" />
          <span className="agent-orb-core relative h-3 w-3 rounded-full" />
        </div>
        <div className="min-w-0">
          <p className="font-mono text-[10px] uppercase tracking-[0.16em] text-ink-3">
            Welcome back
          </p>
          <div className="mt-0.5 flex min-w-0 items-baseline gap-2">
            <h1
              className="truncate font-display text-base font-semibold tracking-tight text-ink"
              title={agent.name}
            >
              {agent.name}
            </h1>
            <span className="rounded-full border border-accent/30 bg-accent/10 px-2 py-0.5 font-mono text-[9px] uppercase tracking-wider text-accent">
              Main
            </span>
          </div>
          <div className="mt-2 hidden flex-wrap gap-1.5 sm:flex">
            <StatusLabel
              icon={online ? <BoltIcon size={11} /> : <AlertIcon size={11} />}
              iconLabel={`Daemon ${online ? 'online' : 'offline'}`}
              iconClassName={online ? 'text-mint' : 'text-danger'}
            >
              Daemon {online ? 'Online' : 'Offline'}
            </StatusLabel>
            <StatusLabel
              icon={<SparkIcon size={11} />}
              iconLabel={`Agent ${agentStatus.toLowerCase()}`}
              iconClassName="text-accent"
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
          <div
            data-testid="compact-daemon-status"
            className="mt-1.5 inline-flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-2 sm:hidden"
          >
            <span aria-hidden className={online ? 'text-mint' : 'text-danger'}>
              {online ? <BoltIcon size={11} /> : <AlertIcon size={11} />}
            </span>
            <span>
              <span>Daemon</span> <span>{online ? 'Online' : 'Offline'}</span>
            </span>
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
