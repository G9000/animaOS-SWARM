import { useEffect, useRef, useState, type ReactNode } from 'react';
import type { DaemonConnection } from '../hooks/useDaemonBootstrap';
import type { DaemonWorkspaceState } from '../lib/daemon-api';
import type { HashPage, HashRoute, Navigate } from '../lib/hash-route';
import type { AgentDetail } from '../lib/types';
import { AgentPresence } from './AgentPresence';
import { WorkspaceHub } from './WorkspaceHub';
import { WorkspaceFiles } from './WorkspaceFiles';
import { WorkspaceCapabilities } from './WorkspaceCapabilities';
import { CommandMenu, type StudioCommand } from './CommandMenu';
import { PROMPT_LIBRARY } from '../lib/prompt-library';
import { GearIcon, PulseIcon, SendIcon, SparkIcon } from './icons';
import { ghostBtnCls } from './ui-bits';

/** Pages this release renders; the other hash pages open the conversation
 *  until their milestones build them. */
export const AVAILABLE_PAGES = [
  'work',
  'files',
  'connectors',
  'capabilities',
] as const satisfies readonly HashPage[];
export type AvailablePage = (typeof AVAILABLE_PAGES)[number];

interface Destination {
  page: AvailablePage;
  label: string;
  icon: ReactNode;
}

const PRIMARY_DESTINATIONS: Destination[] = [
  { page: 'work', label: 'Work', icon: <SparkIcon size={16} /> },
  { page: 'files', label: 'Files', icon: <PulseIcon size={16} /> },
  { page: 'connectors', label: 'Connectors', icon: <GearIcon size={16} /> },
];
const SYSTEM_DESTINATIONS: Destination[] = [
  { page: 'capabilities', label: 'Capabilities', icon: <SparkIcon size={16} /> },
];
const DESTINATIONS = [...PRIMARY_DESTINATIONS, ...SYSTEM_DESTINATIONS];

export function availablePage(route: HashRoute): AvailablePage | null {
  return route.kind === 'page' &&
    (AVAILABLE_PAGES as readonly HashPage[]).includes(route.page)
    ? (route.page as AvailablePage)
    : null;
}

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
  page,
  navigate,
  placement,
  onOpenChats,
}: {
  page: AvailablePage | null;
  navigate: Navigate;
  placement: 'sidebar' | 'bottom-dock';
  onOpenChats: () => void;
}) {
  const sidebar = placement === 'sidebar';
  const [systemOpen, setSystemOpen] = useState(false);
  const systemExpanded =
    systemOpen || SYSTEM_DESTINATIONS.some((item) => item.page === page);
  const itemClass = `studio-nav-item inline-flex items-center gap-3 rounded-xl px-3 py-2.5 text-sm transition ${sidebar ? 'w-full justify-start text-left' : 'min-w-16 shrink-0 flex-col gap-1 text-[10px]'}`;
  const destination = (item: Destination) => (
    <button
      key={item.page}
      type="button"
      onClick={() => navigate({ kind: 'page', page: item.page })}
      aria-current={page === item.page ? 'page' : undefined}
      aria-label={item.label}
      className={itemClass}
    >
      {item.icon}
      <span>{item.label}</span>
    </button>
  );
  return (
    <nav
      aria-label="Workspace navigation"
      aria-orientation={sidebar ? 'vertical' : 'horizontal'}
      data-placement={placement}
      className={
        sidebar
          ? 'studio-navigation flex shrink-0 flex-col gap-1 p-3'
          : 'safe-bottom-dock glass-strong absolute inset-x-3 z-30 flex items-center gap-1 overflow-x-auto rounded-2xl p-1.5'
      }
    >
      {!sidebar && (
        <button
          type="button"
          onClick={onOpenChats}
          aria-current={page === null ? 'page' : undefined}
          aria-label="Chats"
          className={itemClass}
        >
          <SendIcon size={16} />
          <span>Chats</span>
        </button>
      )}
      {PRIMARY_DESTINATIONS.map((item) => destination(item))}
      {sidebar ? (
        <>
          <button
            type="button"
            className={itemClass}
            aria-expanded={systemExpanded}
            onClick={() => setSystemOpen((open) => !open)}
          >
            <GearIcon size={16} />
            <span>System</span>
          </button>
          {systemExpanded && SYSTEM_DESTINATIONS.map((item) => destination(item))}
        </>
      ) : (
        SYSTEM_DESTINATIONS.map((item) => destination(item))
      )}
    </nav>
  );
}

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/** The sessions list on mobile: a modal drawer that takes focus, keeps Tab
 *  inside, closes on Escape, and returns focus to its opener. */
function SessionDrawer({
  children,
  onClose,
  onNewChat,
}: {
  children: ReactNode;
  onClose: () => void;
  onNewChat: () => void;
}) {
  const panel = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const opener =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    panel.current?.querySelector<HTMLElement>(FOCUSABLE)?.focus();
    return () => {
      if (opener?.isConnected) opener.focus();
    };
  }, []);
  return (
    <div
      className="session-drawer"
      role="dialog"
      aria-modal="true"
      aria-label="Sessions"
      onKeyDown={(event) => {
        if (event.key === 'Escape') {
          event.preventDefault();
          event.stopPropagation();
          onClose();
          return;
        }
        if (event.key !== 'Tab') return;
        const focusable = Array.from(
          panel.current?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? [],
        );
        if (focusable.length === 0) return;
        const first = focusable[0];
        const last = focusable[focusable.length - 1];
        if (event.shiftKey && document.activeElement === first) {
          event.preventDefault();
          last.focus();
        } else if (!event.shiftKey && document.activeElement === last) {
          event.preventDefault();
          first.focus();
        }
      }}
    >
      <div ref={panel} className="session-drawer-panel studio-sidebar">
        <div className="flex items-center justify-between gap-2 p-3">
          <button type="button" className={ghostBtnCls} onClick={onNewChat}>
            New chat
          </button>
          <button
            type="button"
            className="studio-tool-button"
            aria-label="Close sessions"
            onClick={onClose}
          >
            ×
          </button>
        </div>
        {children}
      </div>
      <div className="session-drawer-backdrop" aria-hidden onClick={onClose} />
    </div>
  );
}

export function WorkspaceShell({
  mainAgent,
  agents,
  connection,
  route,
  navigate,
  conversation,
  conversationRoute,
  sidebar = null,
  connectors = null,
  workspaceState = null,
  onOpenSettings,
  onChangeWorkspaceAvatar = ignoreWorkspaceAvatarChange,
  onPickPrompt,
  onNewChat,
}: {
  mainAgent: AgentDetail;
  agents: readonly AgentDetail[];
  connection: Exclude<DaemonConnection, 'unknown'>;
  route: HashRoute;
  navigate: Navigate;
  /** The chat or session view; kept mounted so drafts and scroll survive page visits. */
  conversation: ReactNode;
  /** The chat or session a page returns to; defaults to the last one visited. */
  conversationRoute?: HashRoute;
  /** The sessions list for the desktop sidebar and the mobile drawer. */
  sidebar?: ReactNode | null;
  connectors?: ReactNode | null;
  workspaceState?: DaemonWorkspaceState | null;
  onOpenSettings: () => void;
  onChangeWorkspaceAvatar?: (file: File) => Promise<void>;
  onPickPrompt?: (prompt: string) => void;
  onNewChat?: () => void;
}) {
  const page = availablePage(route);
  const [visitedConversation, setVisitedConversation] = useState<HashRoute>(
    route.kind === 'session' ? route : { kind: 'home' },
  );
  const lastConversation = conversationRoute ?? visitedConversation;
  const [commandsOpen, setCommandsOpen] = useState(false);
  const [focusMode, setFocusMode] = useState(false);
  const [drawerOpen, setDrawerOpen] = useState(false);
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
  const newChat = onNewChat ?? (() => navigate({ kind: 'home' }));
  const openConversation = () => navigate(lastConversation);

  useEffect(() => {
    if (route.kind === 'session' || route.kind === 'home')
      setVisitedConversation(route);
    setDrawerOpen(false);
  }, [route]);

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

  const commands: StudioCommand[] = [
    {
      id: 'new-chat',
      title: 'New chat',
      description: 'Start a fresh conversation',
      group: 'Navigate',
      run: newChat,
    },
    ...DESTINATIONS.map((item) => ({
      id: item.page,
      title: `Go to ${item.label}`,
      description: `Open ${item.label.toLowerCase()}`,
      group: 'Navigate',
      run: () => navigate({ kind: 'page', page: item.page }),
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
            if (page !== null) openConversation();
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
              <div className="shrink-0 px-3 pt-3">
                <button
                  type="button"
                  className={`${ghostBtnCls} w-full justify-center`}
                  onClick={newChat}
                >
                  New chat
                </button>
              </div>
              <DestinationNavigation
                page={page}
                navigate={navigate}
                placement="sidebar"
                onOpenChats={openConversation}
              />
              {sidebar}
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
              {!desktopNavigation && sidebar !== null && (
                <button
                  type="button"
                  className="studio-tool-button"
                  aria-label="Open sessions"
                  aria-expanded={drawerOpen}
                  onClick={() => setDrawerOpen(true)}
                >
                  ☰
                </button>
              )}
              <div className="studio-breadcrumb">
                <strong>
                  {page
                    ? DESTINATIONS.find((item) => item.page === page)?.label
                    : mainAgent.name}
                </strong>
                {page === null && (
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
                {page !== null && (
                  <button
                    type="button"
                    className="studio-tool-button"
                    aria-label="Open companion chat"
                    onClick={openConversation}
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
                    aria-label={focusMode ? 'Exit focus mode' : 'Enter focus mode'}
                    aria-pressed={focusMode}
                  >
                    {focusMode ? '↙' : '⛶'}
                  </button>
                )}
              </div>
            </div>
            <div className="studio-view">
              {/* Keep the conversation mounted so drafts and scroll survive page visits. */}
              <div className="companion-chat-panel" hidden={page !== null}>
                {conversation}
              </div>
              {page === 'connectors' ? (
                connectors
              ) : page === 'files' ? (
                <WorkspaceFiles online={connection === 'online'} />
              ) : page === 'capabilities' ? (
                <WorkspaceCapabilities online={connection === 'online'} />
              ) : page === 'work' ? (
                <WorkspaceHub agents={[mainAgent]} initialSection="Tasks" />
              ) : null}
            </div>
            {drawerOpen && !desktopNavigation && sidebar !== null && (
              <SessionDrawer
                onClose={() => setDrawerOpen(false)}
                onNewChat={() => {
                  setDrawerOpen(false);
                  newChat();
                }}
              >
                {sidebar}
              </SessionDrawer>
            )}
          </main>
        </div>
        {!desktopNavigation && (
          <DestinationNavigation
            page={page}
            navigate={navigate}
            placement="bottom-dock"
            onOpenChats={openConversation}
          />
        )}
      </div>
      {commandsOpen && (
        <CommandMenu commands={commands} close={() => setCommandsOpen(false)} />
      )}
    </>
  );
}
