import { useEffect, useState, type ReactNode } from 'react';
import type { DaemonConnection } from '../hooks/useDaemonBootstrap';
import type { DaemonWorkspaceState } from '../lib/daemon-api';
import type { AgentDetail } from '../lib/types';
import { AgentPresence } from './AgentPresence';
import { WorkspaceHub } from './WorkspaceHub';
import { WorkspaceFiles } from './WorkspaceFiles';
import { WorkspaceCapabilities } from './WorkspaceCapabilities';
import { CommandMenu, type StudioCommand } from './CommandMenu';
import { PROMPT_LIBRARY } from '../lib/prompt-library';
import { GearIcon, PulseIcon, SendIcon, SparkIcon } from './icons';
import { ghostBtnCls } from './ui-bits';

export type WorkspaceDestination =
  | 'workspace'
  | 'hub'
  | 'files'
  | 'connectors'
  | 'telegram'
  | 'activity'
  | 'capabilities';

const DESTINATIONS: Array<{
  id: WorkspaceDestination;
  label: string;
  icon: ReactNode;
}> = [
  { id: 'workspace', label: 'Chat', icon: <SendIcon size={16} /> },
  { id: 'hub', label: 'Work', icon: <SparkIcon size={16} /> },
  { id: 'files', label: 'Files', icon: <PulseIcon size={16} /> },
  { id: 'connectors', label: 'Connectors', icon: <GearIcon size={16} /> },
  { id: 'activity', label: 'Activity', icon: <PulseIcon size={16} /> },
  { id: 'capabilities', label: 'Capabilities', icon: <SparkIcon size={16} /> },
];
const ignoreWorkspaceAvatarChange = async () => undefined;
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
          icon: <SendIcon size={16} />,
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
          : 'safe-bottom-dock glass-strong absolute inset-x-3 z-30 flex items-center gap-1 overflow-x-auto rounded-2xl p-1.5'
      }
    >
      {destinations.map((item) => (
        <button
          key={item.id}
          type="button"
          onClick={() => setDestination(item.id)}
          aria-current={destination === item.id ? 'page' : undefined}
          aria-label={item.label}
          className={`studio-nav-item inline-flex items-center gap-3 rounded-xl px-3 py-2.5 text-sm transition ${sidebar ? 'w-full justify-start text-left' : 'min-w-16 shrink-0 flex-col gap-1 text-[10px]'}`}
        >
          {item.icon}
          <span>{item.label}</span>
        </button>
      ))}
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
  const [destination, setDestination] =
    useState<WorkspaceDestination>('workspace');
  const [commandsOpen, setCommandsOpen] = useState(false);
  const [focusMode, setFocusMode] = useState(false);
  const desktopNavigation = useDesktopNavigation();
  const companyName = workspaceState?.configured
    ? (workspaceState.workspace?.companyName ?? null)
    : null;
  const hasAvatar =
    workspaceState?.configured === true &&
    workspaceState.workspace?.hasAvatar === true;
  const workingHelpers = agents.filter(
    (agent) => agent.id !== mainAgent.id && agent.status === 'Running',
  ).length;

  useEffect(() => {
    const shortcut = (event: KeyboardEvent) => {
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
    window.addEventListener('keydown', shortcut);
    return () => window.removeEventListener('keydown', shortcut);
  }, []);
  useEffect(() => {
    if (destination === 'telegram' && telegram === null)
      setDestination('workspace');
  }, [destination, telegram]);

  const commands: StudioCommand[] = [
    ...[
      ...DESTINATIONS,
      ...(telegram !== null
        ? [{ id: 'telegram' as const, label: 'Telegram' }]
        : []),
    ].map((item) => ({
      id: item.id,
      title: `Go to ${item.label}`,
      description: `Open ${item.label.toLowerCase()}`,
      group: 'Navigate',
      run: () => setDestination(item.id),
    })),
    {
      id: 'settings',
      title: 'Companion settings',
      description: 'Identity, model, and access',
      group: 'Navigate',
      run: () => requestAnimationFrame(onOpenSettings),
    },
    ...(desktopNavigation
      ? [
          {
            id: 'focus',
            title: focusMode ? 'Exit focus mode' : 'Enter focus mode',
            description: 'More room for your conversation',
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

  return (
    <>
      <div
        className={`studio-shell companion-shell relative z-[1] flex min-h-0 flex-1 flex-col ${focusMode ? 'is-focused' : ''}`}
        inert={commandsOpen || undefined}
        aria-hidden={commandsOpen || undefined}
      >
        {!desktopNavigation && (
          <AgentPresence
            agent={mainAgent}
            connection={connection}
            companyName={companyName}
            placement="mobile-bar"
            hasAvatar={hasAvatar}
            onChangeWorkspaceAvatar={onChangeWorkspaceAvatar}
            onOpenSettings={onOpenSettings}
          />
        )}
        <div className="studio-frame relative flex min-h-0 flex-1">
          {desktopNavigation && !focusMode && (
            <aside className="studio-sidebar relative z-20 flex w-60 shrink-0 flex-col border-r border-line">
              <div className="studio-brand">
                <span className="studio-brand-mark" aria-hidden>
                  ✳
                </span>
                <span>Anima</span>
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
              <div className="companion-status" role="status">
                <p>
                  {connection === 'offline'
                    ? 'Cannot reach your companion'
                    : mainAgent.status === 'Running'
                      ? 'Working on your request'
                      : 'Ready when you are'}
                </p>
                <span>
                  {connection === 'offline'
                    ? 'Check the server connection.'
                    : workingHelpers > 0
                      ? `${workingHelpers} ${workingHelpers === 1 ? 'helper is' : 'helpers are'} working`
                      : 'Your conversations stay with you.'}
                </span>
              </div>
              <div className="border-t border-line p-3">
                <button
                  type="button"
                  onClick={onOpenSettings}
                  className={`${ghostBtnCls} w-full justify-start`}
                  aria-label="Settings"
                  title={`Settings for ${mainAgent.name}`}
                >
                  <GearIcon size={15} />
                  <span>Settings</span>
                </button>
              </div>
            </aside>
          )}
          <main className="studio-main spatial-canvas workspace-mobile-safe relative min-h-0 min-w-0 flex-1">
            <div className="studio-topbar">
              <div className="studio-breadcrumb">
                <strong>
                  {destination === 'workspace'
                    ? mainAgent.name
                    : destination === 'telegram'
                      ? 'Telegram'
                      : DESTINATIONS.find((item) => item.id === destination)
                          ?.label}
                </strong>
                {destination === 'workspace' && (
                  <span className="companion-model">{mainAgent.model}</span>
                )}
              </div>
              <span
                className={`studio-connection ${connection === 'online' ? 'is-online' : 'is-offline'}`}
              >
                <i aria-hidden />
                {connection === 'online' ? 'Connected' : 'Offline'}
              </span>
              <div className="studio-topbar-actions">
                {destination !== 'workspace' && (
                  <button
                    type="button"
                    className="studio-tool-button"
                    aria-label="Open companion chat"
                    onClick={() => setDestination('workspace')}
                  >
                    Back to chat
                  </button>
                )}
                <button
                  type="button"
                  className="studio-command-trigger"
                  onClick={() => setCommandsOpen(true)}
                  aria-label="Open command menu"
                >
                  <span aria-hidden>⌕</span>
                  <span className="studio-command-trigger-label">Search</span>
                  <kbd>Ctrl K</kbd>
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
                  >
                    {focusMode ? '↙' : '⛶'}
                  </button>
                )}
              </div>
            </div>
            <div className="studio-view">
              {/* Keep chat mounted so drafts, scroll position, and tool approvals survive navigation. */}
              <div
                className="companion-chat-panel"
                hidden={destination !== 'workspace'}
              >
                {workspace}
              </div>
              {destination === 'telegram' ? (
                telegram
              ) : destination === 'connectors' ? (
                connectors
              ) : destination === 'activity' ? (
                activity
              ) : destination === 'files' ? (
                <WorkspaceFiles online={connection === 'online'} />
              ) : destination === 'capabilities' ? (
                <WorkspaceCapabilities online={connection === 'online'} />
              ) : destination === 'hub' ? (
                <WorkspaceHub agents={[mainAgent]} initialSection="Tasks" />
              ) : null}
            </div>
          </main>
        </div>
        {!desktopNavigation && (
          <DestinationNavigation
            destination={destination}
            setDestination={setDestination}
            placement="bottom-dock"
            hasTelegram={telegram !== null}
          />
        )}
      </div>
      {commandsOpen && (
        <CommandMenu commands={commands} close={() => setCommandsOpen(false)} />
      )}
    </>
  );
}
