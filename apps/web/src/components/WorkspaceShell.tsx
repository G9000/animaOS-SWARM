import { useEffect, useState, type ReactNode } from 'react';

import type { DaemonConnection } from '../hooks/useDaemonBootstrap';
import type { DaemonWorkspaceState } from '../lib/daemon-api';
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
  placement: 'sidebar' | 'bottom-dock';
  hasTelegram: boolean;
}) {
  const sidebar = placement === 'sidebar';
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
      aria-orientation={sidebar ? 'vertical' : 'horizontal'}
      data-placement={placement}
      className={
        sidebar
          ? 'relative z-20 flex w-48 shrink-0 flex-col gap-1 border-r border-line bg-panel/55 p-3 backdrop-blur-2xl xl:w-56'
          : 'safe-bottom-dock glass-strong absolute inset-x-3 z-30 flex items-center justify-around gap-1 rounded-2xl p-1.5 shadow-2xl shadow-black/50'
      }
    >
      {sidebar ? (
        <p className="mb-2 px-3 font-mono text-[9px] uppercase tracking-[0.18em] text-ink-3">
          Navigate
        </p>
      ) : null}
      {destinations.map((item) => {
        const active = destination === item.id;
        return (
          <button
            key={item.id}
            type="button"
            onClick={() => setDestination(item.id)}
            aria-current={active ? 'page' : undefined}
            className={`inline-flex min-w-0 items-center gap-2 rounded-xl px-3 py-2 text-xs font-medium transition ${
              sidebar
                ? 'w-full justify-start py-2.5 text-left'
                : 'flex-1 justify-center'
            } ${
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
  workspaceState = null,
  onOpenSettings,
}: {
  mainAgent: AgentDetail;
  agents: readonly AgentDetail[];
  connection: Exclude<DaemonConnection, 'unknown'>;
  workspace: ReactNode;
  activity: ReactNode;
  telegram?: ReactNode | null;
  workspaceState?: DaemonWorkspaceState | null;
  onOpenSettings: () => void;
}) {
  const [destination, setDestination] =
    useState<WorkspaceDestination>('workspace');
  const desktopNavigation = useDesktopNavigation();
  const companyName =
    workspaceState?.configured && workspaceState.workspace !== null
      ? workspaceState.workspace.companyName
      : null;

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
        companyName={companyName}
        onOpenSettings={onOpenSettings}
      />

      <div className="relative flex min-h-0 flex-1">
        {desktopNavigation ? (
          <DestinationNavigation
            destination={destination}
            setDestination={setDestination}
            placement="sidebar"
            hasTelegram={telegram !== null}
          />
        ) : null}

        <main className="spatial-canvas workspace-mobile-safe relative min-h-0 min-w-0 flex-1">
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
      </div>

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
