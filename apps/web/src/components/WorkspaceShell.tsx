import { useEffect, useState, type ReactNode } from 'react';

import type { DaemonConnection } from '../hooks/useDaemonBootstrap';
import type { DaemonWorkspaceState } from '../lib/daemon-api';
import type { AgentDetail } from '../lib/types';
import { AgentPresence } from './AgentPresence';
import { AgentsView } from './AgentsView';
import { CommandMenu, type StudioCommand } from './CommandMenu';
import { PROMPT_LIBRARY } from '../lib/prompt-library';
import { AgentsIcon, GearIcon, PulseIcon, SendIcon, SparkIcon } from './icons';
import { formatTokens, ghostBtnCls } from './ui-bits';

export type WorkspaceDestination =
  | 'workspace'
  | 'connectors'
  | 'telegram'
  | 'activity'
  | 'agents';

const ignoreWorkspaceAvatarChange = async () => undefined;

const DESTINATIONS: Array<{
  id: WorkspaceDestination;
  label: string;
  icon: ReactNode;
}> = [
  { id: 'workspace', label: 'Workspace', icon: <SparkIcon size={15} /> },
  { id: 'connectors', label: 'Connectors', icon: <GearIcon size={15} /> },
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
          ? 'studio-navigation flex min-h-0 flex-1 flex-col gap-1 p-3'
          : 'safe-bottom-dock glass-strong absolute inset-x-3 z-30 flex items-center justify-around gap-1 rounded-2xl p-1.5 shadow-2xl shadow-black/50'
      }
    >
      {sidebar ? (
        <p className="mb-2 px-3 font-mono text-[9px] uppercase tracking-[0.18em] text-ink-3">
          Your space
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
            aria-label={item.label}
            className={`studio-nav-item inline-flex min-w-0 items-center gap-2 rounded-xl px-3 py-2 text-xs font-medium transition ${
              sidebar
                ? 'w-full justify-start py-2.5 text-left'
                : 'flex-1 flex-col justify-center gap-1 px-1 text-[10px]'
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
  connectors = null,
  workspaceState = null,
  onOpenSettings,
  onChangeWorkspaceAvatar = ignoreWorkspaceAvatarChange,
  onPickPrompt,
}: {
  mainAgent: AgentDetail;
  agents: readonly AgentDetail[];
  connection: Exclude<DaemonConnection, 'unknown'>;
  workspace: ReactNode;
  activity: ReactNode;
  telegram?: ReactNode | null;
  connectors?: ReactNode | null;
  workspaceState?: DaemonWorkspaceState | null;
  onOpenSettings: () => void;
  onChangeWorkspaceAvatar?: (file: File) => Promise<void>;
  onPickPrompt?: (prompt: string) => void;
}) {
  const [commandsOpen, setCommandsOpen] = useState(false);
  const [focusMode, setFocusMode] = useState(false);
  const [destination, setDestination] =
    useState<WorkspaceDestination>('workspace');
  const desktopNavigation = useDesktopNavigation();
  const companyName =
    workspaceState?.configured && workspaceState.workspace !== null
      ? workspaceState.workspace.companyName
      : null;
  const hasAvatar =
    workspaceState?.configured === true &&
    workspaceState.workspace?.hasAvatar === true;

  useEffect(() => {
    const handleShortcut = (event: KeyboardEvent) => {
      if (
        (event.ctrlKey || event.metaKey) &&
        event.key.toLowerCase() === 'k' &&
        !event.isComposing
      ) {
        if (document.querySelector('[aria-modal="true"]')) return;
        event.preventDefault();
        setCommandsOpen(true);
      }
    };
    window.addEventListener('keydown', handleShortcut);
    return () => window.removeEventListener('keydown', handleShortcut);
  }, []);

  const commands: StudioCommand[] = [
    ...[
      ...DESTINATIONS,
      ...(telegram !== null
        ? [{ id: 'telegram' as const, label: 'Telegram' }]
        : []),
    ].map((item) => ({
      id: item.id,
      title: `Go to ${item.label}`,
      description:
        item.id === 'activity'
          ? 'Schedules, check-ins, and token usage'
          : `Open your ${item.label.toLowerCase()}`,
      group: 'Navigate',
      run: () => setDestination(item.id),
    })),
    {
      id: 'settings',
      title: 'Agent settings',
      description: 'Identity, model, and access',
      group: 'Navigate',
      run: () => requestAnimationFrame(onOpenSettings),
    },
    ...(desktopNavigation
      ? [
          {
            id: 'focus',
            title: focusMode ? 'Exit focus mode' : 'Enter focus mode',
            description: 'Give your work more room',
            group: 'View',
            run: () => setFocusMode((value) => !value),
          },
        ]
      : []),
    ...(onPickPrompt
      ? PROMPT_LIBRARY.map((prompt) => ({
          id: prompt.id,
          title: prompt.title,
          description: prompt.description,
          group: prompt.category,
          run: () => {
            setDestination('workspace');
            onPickPrompt(prompt.prompt);
            requestAnimationFrame(() =>
              document
                .querySelector<HTMLTextAreaElement>('[data-workspace-composer]')
                ?.focus(),
            );
          },
        }))
      : []),
  ];

  useEffect(() => {
    if (destination === 'telegram' && telegram === null) {
      setDestination('workspace');
    }
  }, [destination, telegram]);

  return (
    <>
      <div
        className={`studio-shell relative z-[1] flex min-h-0 flex-1 flex-col ${focusMode ? 'is-focused' : ''}`}
        inert={commandsOpen || undefined}
        aria-hidden={commandsOpen || undefined}
      >
        {!desktopNavigation ? (
          <AgentPresence
            agent={mainAgent}
            connection={connection}
            companyName={companyName}
            placement="mobile-bar"
            hasAvatar={hasAvatar}
            onChangeWorkspaceAvatar={onChangeWorkspaceAvatar}
            onOpenSettings={onOpenSettings}
          />
        ) : null}

        <div className="studio-frame relative flex min-h-0 flex-1">
          {desktopNavigation && !focusMode ? (
            <aside className="studio-sidebar relative z-20 flex w-56 shrink-0 flex-col border-r border-line bg-panel/55 backdrop-blur-2xl xl:w-64">
              <div className="studio-brand">
                <span className="studio-brand-mark" aria-hidden>
                  ✳
                </span>
                <span>
                  anima<span className="studio-brand-suffix">OS</span>
                </span>
                <span className="studio-edition">STUDIO / 01</span>
              </div>
              <AgentPresence
                agent={mainAgent}
                connection={connection}
                companyName={companyName}
                placement="sidebar"
                hasAvatar={hasAvatar}
                onChangeWorkspaceAvatar={onChangeWorkspaceAvatar}
              />
              <DestinationNavigation
                destination={destination}
                setDestination={setDestination}
                placement="sidebar"
                hasTelegram={telegram !== null}
              />
              <div className="studio-sidebar-note">
                <span className="studio-note-label">WORKSPACE PULSE</span>
                <p className="studio-live-status">
                  <i aria-hidden />
                  {mainAgent.status === 'Running'
                    ? 'Thinking with you'
                    : connection === 'offline'
                      ? 'Connection paused'
                      : 'Ready when you are'}
                </p>
                <div className="studio-sidebar-metrics">
                  <span>
                    <strong>{mainAgent.messages.length}</strong> messages
                  </span>
                  <span>
                    <strong>
                      {formatTokens(mainAgent.token_usage.total_tokens)}
                    </strong>{' '}
                    tokens
                  </span>
                </div>
                <span>
                  {mainAgent.provider} / {mainAgent.model}
                </span>
              </div>
              <div className="border-t border-line/60 p-3">
                <button
                  type="button"
                  onClick={onOpenSettings}
                  className={`${ghostBtnCls} w-full justify-start`}
                  aria-label="Settings"
                  title={`Settings for ${mainAgent.name}`}
                >
                  <GearIcon size={13} />
                  <span>Settings</span>
                </button>
              </div>
            </aside>
          ) : null}

          <main className="studio-main spatial-canvas workspace-mobile-safe relative min-h-0 min-w-0 flex-1">
            <div className="studio-topbar">
              <div className="studio-breadcrumb">
                <span>{companyName || 'Personal space'}</span>
                <span aria-hidden>/</span>
                <strong>
                  {destination === 'telegram'
                    ? 'Telegram'
                    : destination.charAt(0).toUpperCase() +
                      destination.slice(1)}
                </strong>
              </div>
              <span
                className={`studio-connection ${connection === 'online' ? 'is-online' : 'is-offline'}`}
              >
                <i aria-hidden />
                {connection === 'online' ? 'Connected' : 'Offline'}
              </span>
              <div className="studio-topbar-actions">
                <button
                  type="button"
                  className="studio-command-trigger"
                  onClick={() => setCommandsOpen(true)}
                  aria-label="Open command menu"
                >
                  <span aria-hidden>⌕</span>
                  <span className="studio-command-trigger-label">Commands</span>
                  <kbd>⌘ / Ctrl K</kbd>
                </button>
                {desktopNavigation && (
                  <button
                    type="button"
                    className="studio-tool-button"
                    onClick={() => setFocusMode((value) => !value)}
                    aria-label={
                      focusMode ? 'Exit focus mode' : 'Enter focus mode'
                    }
                    aria-pressed={focusMode}
                    title={focusMode ? 'Exit focus mode' : 'Enter focus mode'}
                  >
                    {focusMode ? '↙' : '⛶'}
                  </button>
                )}
              </div>
            </div>
            <div className="studio-view">
              {destination === 'workspace' ? (
                workspace
              ) : destination === 'telegram' && telegram !== null ? (
                telegram
              ) : destination === 'connectors' ? (
                connectors
              ) : destination === 'activity' ? (
                activity
              ) : (
                <AgentsView agents={agents} mainAgent={mainAgent} />
              )}
            </div>
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
      {commandsOpen && (
        <CommandMenu commands={commands} close={() => setCommandsOpen(false)} />
      )}
    </>
  );
}
