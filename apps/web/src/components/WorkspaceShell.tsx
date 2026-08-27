import { useEffect, useState, type ReactNode } from 'react';

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

const DESKTOP_NAVIGATION_QUERY = '(min-width: 768px)';

function useDesktopNavigation() {
  const [desktop, setDesktop] = useState(
    () =>
      typeof window.matchMedia !== 'function' ||
      window.matchMedia(DESKTOP_NAVIGATION_QUERY).matches,
  );

  useEffect(() => {
    if (typeof window.matchMedia !== 'function') return;
    const media = window.matchMedia(DESKTOP_NAVIGATION_QUERY);
    const update = () => setDesktop(media.matches);
    update();
    media.addEventListener('change', update);
    return () => media.removeEventListener('change', update);
  }, []);

  return desktop;
}

function DestinationNavigation({
  destination,
  setDestination,
  placement,
}: {
  destination: WorkspaceDestination;
  setDestination: (destination: WorkspaceDestination) => void;
  placement: 'top' | 'bottom-dock';
}) {
  return (
    <nav
      aria-label="Workspace navigation"
      data-placement={placement}
      className={
        placement === 'top'
          ? 'flex items-center justify-center gap-1 border-b border-line bg-panel/35 p-1.5 backdrop-blur-xl'
          : 'fixed inset-x-3 bottom-3 z-30 flex items-center justify-around gap-1 rounded-2xl border border-line bg-panel/95 p-1.5 shadow-2xl shadow-black/40 backdrop-blur-xl'
      }
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
  );
}

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
  const desktopNavigation = useDesktopNavigation();

  return (
    <div className="relative z-[1] flex min-h-0 flex-1 flex-col">
      <AgentPresence
        agent={mainAgent}
        connection={connection}
        onOpenSettings={onOpenSettings}
      />

      {desktopNavigation ? (
        <DestinationNavigation
          destination={destination}
          setDestination={setDestination}
          placement="top"
        />
      ) : null}

      <main
        className={`relative min-h-0 flex-1 ${desktopNavigation ? 'pb-0' : 'pb-20'}`}
      >
        {destination === 'workspace' ? (
          workspace
        ) : destination === 'activity' ? (
          activity
        ) : (
          <AgentsView agents={agents} mainAgent={mainAgent} />
        )}
      </main>

      {!desktopNavigation ? (
        <DestinationNavigation
          destination={destination}
          setDestination={setDestination}
          placement="bottom-dock"
        />
      ) : null}
    </div>
  );
}
