import type { DaemonConnection } from '../hooks/useDaemonBootstrap';
import type { AgentDetail } from '../lib/types';
import { GearIcon } from './icons';
import { ghostBtnCls } from './ui-bits';
import { WorkspaceAvatar } from './WorkspaceAvatar';

function AgentIdentity({
  agent,
  companyName,
}: {
  agent: AgentDetail;
  companyName: string | null;
}) {
  return (
    <div className="min-w-0">
      <div className="flex min-w-0 items-baseline gap-2">
        <p className="shrink-0 font-mono text-[10px] uppercase tracking-[0.16em] text-ink-3">
          Welcome back
        </p>
        {companyName ? (
          <p
            className="truncate font-mono text-[10px] uppercase tracking-[0.16em] text-ink-2"
            title={companyName}
          >
            {companyName}
          </p>
        ) : null}
      </div>
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
    </div>
  );
}

export function AgentPresence({
  agent,
  companyName = null,
  placement,
  hasAvatar,
  onChangeWorkspaceAvatar,
  onOpenSettings,
}: {
  agent: AgentDetail;
  connection: Exclude<DaemonConnection, 'unknown'>;
  companyName?: string | null;
  placement: 'sidebar' | 'mobile-bar';
  hasAvatar: boolean;
  onChangeWorkspaceAvatar(file: File): Promise<void>;
  onOpenSettings?: () => void;
}) {
  if (placement === 'mobile-bar') {
    return (
      <header className="studio-mobile-presence relative flex min-w-0 items-center justify-between gap-3 border-b border-line bg-abyss/65 px-4 py-2 backdrop-blur-2xl">
        <div className="flex min-w-0 items-center gap-2.5">
          <WorkspaceAvatar
            placement="mobile-bar"
            hasAvatar={hasAvatar}
            uploadAvatar={onChangeWorkspaceAvatar}
          />
          <h1
            className="truncate font-display text-sm font-semibold tracking-tight text-ink"
            title={agent.name}
          >
            {agent.name}
          </h1>
        </div>
        {onOpenSettings ? (
          <button
            type="button"
            onClick={onOpenSettings}
            className={`${ghostBtnCls} shrink-0`}
            aria-label="Settings"
            title={`Settings for ${agent.name}`}
          >
            <GearIcon size={13} />
            <span className="sr-only">Settings</span>
          </button>
        ) : null}
      </header>
    );
  }

  return (
    <div className="studio-presence flex items-center gap-3 border-b border-line/60 p-3">
      <WorkspaceAvatar
        placement="sidebar"
        hasAvatar={hasAvatar}
        uploadAvatar={onChangeWorkspaceAvatar}
      />
      <AgentIdentity agent={agent} companyName={companyName} />
    </div>
  );
}
