import { useEffect, useState, type ReactNode } from 'react';

import type { DaemonConnection } from '../hooks/useDaemonBootstrap';
import type { AgentDetail } from '../lib/types';
import { AgentPresence } from './AgentPresence';
import { AgentsView } from './AgentsView';
import { AgentsIcon, PulseIcon, SendIcon, SparkIcon } from './icons';

export type WorkspaceDestination =
  | 'workspace'
  | 'telegram'
  | 'activity'
  | 'agents';

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
  hasTelegram,
}: {
  destination: WorkspaceDestination;
  setDestination: (destination: WorkspaceDestination) => void;
  placement: 'top-shell' | 'bottom-dock';
  hasTelegram: boolean;
}) {
  const destinations = hasTelegram
    ? [
        DESTINATIONS[0],
        {
          id: 'telegram' as const,
          label: 'Telegram',
          icon: <SendIcon size={15} />,
        },
        ...DESTINATIONS.slice(1),
      ]
    : DESTINATIONS;
  return (
    <nav
      aria-label="Workspace navigation"
      data-placement={placement}
      className={
        placement === 'top-shell'
          ? 'glass relative z-20 mx-auto mt-2 flex w-[min(calc(100%-2rem),40rem)] items-center justify-center gap-1 rounded-2xl p-1.5'
          : 'safe-bottom-dock glass-strong absolute inset-x-3 z-30 flex items-center justify-around gap-1 rounded-2xl p-1.5 shadow-2xl shadow-black/50'
      }
    >
      {destinations.map((item) => {
        const active = destination === item.id;
        return (
          <button
            key={item.id}
            type="button"
            onClick={() => setDestination(item.id)}
            aria-current={active ? 'page' : undefined}
            className={`inline-flex min-w-0 flex-1 items-center justify-center gap-2 rounded-xl px-3 py-2 text-xs font-medium transition md:max-w-40 ${
              active
                ? 'bg-accent/12 text-accent shadow-[inset_0_0_0_1px_rgb(var(--color-accent-rgb)/0.18)]'
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
  telegram = null,
  onOpenSettings,
}: {
  mainAgent: AgentDetail;
  agents: readonly AgentDetail[];
  connection: Exclude<DaemonConnection, 'unknown'>;
  workspace: ReactNode;
  activity: ReactNode;
  telegram?: ReactNode | null;
  onOpenSettings: () => void;
}) {
  const [destination, setDestination] =
    useState<WorkspaceDestination>('workspace');
  const desktopNavigation = useDesktopNavigation();

  useEffect(() => {
    if (destination === 'telegram' && telegram === null) {
      setDestination('workspace');
    }
  }, [destination, telegram]);

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
          placement="top-shell"
          hasTelegram={telegram !== null}
        />
      ) : null}

      <main className="spatial-canvas workspace-mobile-safe relative min-h-0 flex-1">
        {destination === 'workspace' ? (
          workspace
        ) : destination === 'telegram' && telegram !== null ? (
          telegram
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
          hasTelegram={telegram !== null}
        />
      ) : null}
    </div>
  );
}
