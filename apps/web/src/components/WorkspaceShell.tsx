import { useState, type ReactNode } from 'react';

import type { DaemonConnection } from '../hooks/useDaemonBootstrap';
import type { AgentDetail } from '../lib/types';
import { AgentPresence } from './AgentPresence';
import { AgentsView } from './AgentsView';
import { AgentsIcon, PulseIcon, SparkIcon } from './icons';

export type WorkspaceDestination = 'workspace' | 'activity' | 'agents';

const DESTINATIONS: Array<{
  id: WorkspaceDestination;
  label: string;
  icon: ReactNode;
}> = [
  { id: 'workspace', label: 'Workspace', icon: <SparkIcon size={15} /> },
  { id: 'activity', label: 'Activity', icon: <PulseIcon size={15} /> },
  { id: 'agents', label: 'Agents', icon: <AgentsIcon size={15} /> },
];

export function WorkspaceShell({
  mainAgent,
  agents,
  connection,
  workspace,
  activity,
  onOpenSettings,
}: {
  mainAgent: AgentDetail;
  agents: readonly AgentDetail[];
  connection: Exclude<DaemonConnection, 'unknown'>;
  workspace: ReactNode;
  activity: ReactNode;
  onOpenSettings: () => void;
}) {
  const [destination, setDestination] =
    useState<WorkspaceDestination>('workspace');

  return (
    <div className="relative z-[1] flex min-h-0 flex-1 flex-col">
      <AgentPresence
        agent={mainAgent}
        connection={connection}
        onOpenSettings={onOpenSettings}
      />

      <nav
        aria-label="Workspace navigation"
        data-desktop-placement="top"
        data-mobile-placement="bottom-dock"
        className="fixed inset-x-3 bottom-3 z-30 flex items-center justify-around gap-1 rounded-2xl border border-line bg-panel/95 p-1.5 shadow-2xl shadow-black/40 backdrop-blur-xl md:static md:inset-auto md:bottom-auto md:justify-center md:rounded-none md:border-x-0 md:border-t-0 md:bg-panel/35 md:shadow-none"
      >
        {DESTINATIONS.map((item) => {
          const active = destination === item.id;
          return (
            <button
              key={item.id}
              type="button"
              onClick={() => setDestination(item.id)}
              aria-current={active ? 'page' : undefined}
              className={`inline-flex min-w-0 flex-1 items-center justify-center gap-2 rounded-xl px-3 py-2 text-xs font-medium transition md:max-w-40 ${
                active
                  ? 'bg-sky-500/12 text-sky-300'
                  : 'text-ink-3 hover:bg-white/[0.04] hover:text-ink'
              }`}
            >
              {item.icon}
              <span>{item.label}</span>
            </button>
          );
        })}
      </nav>

      <main className="relative min-h-0 flex-1 pb-20 md:pb-0">
        {destination === 'workspace' ? (
          workspace
        ) : destination === 'activity' ? (
          activity
        ) : (
          <AgentsView agents={agents} mainAgent={mainAgent} />
        )}
      </main>
    </div>
  );
}
