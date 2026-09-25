import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState, type ComponentProps } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { toolNamesForProfile } from '../lib/agent-access';
import { daemon } from '../lib/daemon-api';
import type { HashRoute } from '../lib/hash-route';
import type { AgentDetail } from '../lib/types';
import { sessionFixture } from '../test/sessions';
import { SessionSidebar } from './sessions/SessionSidebar';
import { WorkspaceShell } from './WorkspaceShell';

function agent(
  id: string,
  name: string,
  createdAt: number,
  overrides: Partial<AgentDetail> = {},
): AgentDetail {
  return {
    id,
    name,
    provider: 'openai',
    model: 'gpt-4.1',
    toolNames: toolNamesForProfile('collaborate'),
    created_at_ms: createdAt,
    status: 'Idle',
    token_usage: {
      prompt_tokens: 3,
      completion_tokens: 5,
      total_tokens: 8,
    },
    messages: [],
    ...overrides,
  };
}

type ShellProps = Partial<ComponentProps<typeof WorkspaceShell>> & {
  initialRoute?: HashRoute;
};

/** Holds the route the way `useHashRoute` does in the app. */
function Shell({ initialRoute = { kind: 'home' }, ...props }: ShellProps) {
  const [route, setRoute] = useState<HashRoute>(initialRoute);
  const main = props.mainAgent ?? agent('agent-main', 'Nova', 1);
  return (
    <WorkspaceShell
      {...props}
      mainAgent={main}
      agents={props.agents ?? [main]}
      connection={props.connection ?? 'online'}
      conversation={props.conversation ?? <div>Workspace canvas</div>}
      onOpenSettings={props.onOpenSettings ?? vi.fn()}
      route={route}
      navigate={(next) => setRoute(next)}
    />
  );
}

function mobile() {
  vi.stubGlobal(
    'matchMedia',
    vi.fn(() => ({
      matches: false,
      media: '(min-width: 768px)',
      onchange: null,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
      addListener: vi.fn(),
      removeListener: vi.fn(),
      dispatchEvent: vi.fn(),
    })),
  );
}

const configuredWorkspace = (hasAvatar: boolean) => ({
  configured: true,
  workspace: {
    rootPath: '/workspaces/northwind',
    companyName: 'Northwind Research',
    mission: 'Map supply chains',
    values: ['rigor'],
    hasAvatar,
  },
  defaultRoot: '/workspaces',
});

beforeEach(() => {
  vi.spyOn(daemon, 'agentJobs').mockResolvedValue([]);
  vi.spyOn(daemon, 'agentTasks').mockResolvedValue({
    tasks: [],
    revision: '1',
  });
  vi.spyOn(daemon, 'listSchedules').mockResolvedValue({ schedules: [] });
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('WorkspaceShell', () => {
  it('lands on the conversation with a New chat action and no work dispatched', async () => {
    const onNewChat = vi.fn();
    render(<Shell onNewChat={onNewChat} />);
    expect(screen.getByText('Workspace canvas')).toBeVisible();
    expect(
      screen.queryByRole('button', { name: 'Operations' }),
    ).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'New chat' }));
    expect(onNewChat).toHaveBeenCalledOnce();
  });

  it('shows the sessions list between the navigation and the status', () => {
    render(<Shell sidebar={<div>Sessions list</div>} />);
    const sidebar = screen.getByRole('complementary');
    expect(within(sidebar).getByText('Sessions list')).toBeVisible();
    expect(
      within(sidebar)
        .getByRole('navigation', { name: 'Workspace navigation' })
        .compareDocumentPosition(within(sidebar).getByText('Sessions list')) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).not.toBe(0);
  });

  it('opens capabilities from the collapsible System group', async () => {
    vi.spyOn(daemon, 'capabilities').mockResolvedValue({
      schemaVersion: 1,
      tools: [],
      persistence: {
        controlPlane: 'file',
        memory: 'file',
        executionJournal: false,
      },
      extensions: [],
      limitations: [],
    });
    render(<Shell />);
    expect(
      screen.queryByRole('button', { name: 'Capabilities', exact: true }),
    ).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'System' }));
    await userEvent.click(
      screen.getByRole('button', { name: 'Capabilities', exact: true }),
    );
    expect(await screen.findByText('Tools follow your authority')).toBeVisible();
    expect(
      screen.getByRole('button', { name: 'Capabilities', exact: true }),
    ).toHaveAttribute('aria-current', 'page');
  });

  it('opens commands with Control K, filters actions and navigates with Enter', async () => {
    const user = userEvent.setup();
    render(<Shell connectors={<div>Manage connections</div>} />);
    await user.keyboard('{Control>}k{/Control}');
    expect(screen.getByRole('dialog', { name: 'Command menu' })).toBeVisible();
    await user.type(
      screen.getByRole('combobox', { name: 'Search commands' }),
      'connectors',
    );
    await user.keyboard('{Enter}');
    expect(screen.getByText('Manage connections')).toBeVisible();
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('can enter and leave focus mode without losing the conversation', async () => {
    const user = userEvent.setup();
    render(<Shell />);
    await user.click(screen.getByRole('button', { name: 'Enter focus mode' }));
    expect(screen.queryByRole('complementary')).not.toBeInTheDocument();
    expect(screen.getByText('Workspace canvas')).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Exit focus mode' }));
    expect(screen.getByRole('complementary')).toBeVisible();
  });

  it('contains keyboard focus in commands and restores the opener on Escape', async () => {
    const user = userEvent.setup();
    render(<Shell />);
    const opener = screen.getByRole('button', { name: 'Open command menu' });
    await user.click(opener);
    const search = screen.getByRole('combobox', { name: 'Search commands' });
    expect(search).toHaveFocus();
    await user.tab();
    expect(
      screen.getByRole('button', { name: 'Close command menu' }),
    ).toHaveFocus();
    await user.tab();
    expect(search).toHaveFocus();
    await user.keyboard('{Escape}');
    expect(opener).toHaveFocus();
  });

  it('inserts a prompt from commands without invoking a send', async () => {
    const user = userEvent.setup();
    const pick = vi.fn();
    render(<Shell onPickPrompt={pick} />);
    await user.click(screen.getByRole('button', { name: 'Open command menu' }));
    await user.type(
      screen.getByRole('combobox', { name: 'Search commands' }),
      'Plan my next hour',
    );
    await user.keyboard('{Enter}');
    expect(pick).toHaveBeenCalledWith(
      expect.stringContaining('Help me plan my next hour'),
    );
    expect(screen.getByText('Workspace canvas')).toBeVisible();
  });

  it('keeps helpers out of the top-level navigation', () => {
    const nova = agent('agent-main', 'Nova', 1);
    render(<Shell mainAgent={nova} agents={[nova, agent('scout', 'Scout', 2)]} />);
    expect(screen.queryByRole('button', { name: 'Team' })).not.toBeInTheDocument();
    expect(
      screen.queryByRole('button', { name: 'Message Scout' }),
    ).not.toBeInTheDocument();
    expect(screen.getByText('Workspace canvas')).toBeVisible();
  });

  it('uses a left sidebar and returns from a page to the mounted conversation', async () => {
    render(<Shell />);
    const navigation = screen.getByRole('navigation', {
      name: 'Workspace navigation',
    });
    expect(navigation).toHaveAttribute('data-placement', 'sidebar');
    expect(navigation).toHaveAttribute('aria-orientation', 'vertical');
    expect(navigation.closest('aside')?.nextElementSibling?.tagName).toBe('MAIN');
    await userEvent.click(within(navigation).getByRole('button', { name: 'Work' }));
    expect(within(navigation).getByRole('button', { name: 'Work' })).toHaveAttribute(
      'aria-current',
      'page',
    );
    expect(screen.getByText('Workspace canvas')).not.toBeVisible();
    await userEvent.click(
      screen.getByRole('button', { name: 'Open companion chat' }),
    );
    expect(screen.getByText('Workspace canvas')).toBeVisible();
  });

  it('shows the conversation for pages that arrive in later releases', () => {
    render(<Shell initialRoute={{ kind: 'page', page: 'approvals' }} />);
    expect(screen.getByText('Workspace canvas')).toBeVisible();
    expect(
      screen.queryByRole('button', { name: 'Open companion chat' }),
    ).not.toBeInTheDocument();
  });

  it('shows the main agent identity in the sidebar presence block', () => {
    render(<Shell connection="offline" />);
    const sidebar = screen.getByRole('complementary');
    expect(within(sidebar).getByRole('heading', { name: 'Nova' })).toBeVisible();
    expect(within(sidebar).getByText('Welcome back')).toBeVisible();
    expect(within(sidebar).getByText('Companion')).toBeVisible();
  });

  it('shows the persisted workspace avatar and uploads a replacement', async () => {
    const user = userEvent.setup();
    const onChangeWorkspaceAvatar = vi.fn().mockResolvedValue(undefined);
    Object.defineProperties(URL, {
      createObjectURL: {
        configurable: true,
        value: vi.fn(() => 'blob:workspace-avatar-preview'),
      },
      revokeObjectURL: { configurable: true, value: vi.fn() },
    });
    render(
      <Shell
        workspaceState={configuredWorkspace(true)}
        onChangeWorkspaceAvatar={onChangeWorkspaceAvatar}
      />,
    );
    expect(
      screen
        .getByRole('button', { name: 'Change workspace avatar' })
        .querySelector('img'),
    ).toHaveAttribute('src', '/api/workspace/avatar?v=0');
    const file = new File(['avatar'], 'avatar.png', { type: 'image/png' });
    await user.upload(screen.getByLabelText('Workspace avatar image file'), file);
    await waitFor(() => expect(onChangeWorkspaceAvatar).toHaveBeenCalledWith(file));
  });

  it('shows a compact presence bar on mobile', () => {
    mobile();
    render(
      <Shell
        mainAgent={agent('agent-main', 'Nova', 1, { status: 'Running' })}
        connection="offline"
      />,
    );
    const bar = screen.getByRole('banner');
    expect(within(bar).getByRole('heading', { name: 'Nova' })).toBeVisible();
    expect(within(bar).getByRole('button', { name: 'Settings' })).toBeVisible();
  });

  it('opens the sessions drawer from the mobile top bar', async () => {
    mobile();
    const user = userEvent.setup();
    render(<Shell sidebar={<div>Sessions list</div>} />);
    expect(screen.queryByText('Sessions list')).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Open sessions' }));
    const drawer = screen.getByRole('dialog', { name: 'Sessions' });
    expect(within(drawer).getByText('Sessions list')).toBeVisible();
    await user.click(within(drawer).getByRole('button', { name: 'Close sessions' }));
    expect(screen.queryByRole('dialog', { name: 'Sessions' })).not.toBeInTheDocument();
  });

  it('moves focus into the sessions drawer, keeps it there, and closes on Escape', async () => {
    mobile();
    const user = userEvent.setup();
    render(<Shell sidebar={<input aria-label="Search sessions" />} />);
    const opener = screen.getByRole('button', { name: 'Open sessions' });
    await user.click(opener);
    const drawer = screen.getByRole('dialog', { name: 'Sessions' });
    const newChat = within(drawer).getByRole('button', { name: 'New chat' });
    const search = within(drawer).getByRole('textbox', {
      name: 'Search sessions',
    });
    expect(newChat).toHaveFocus();
    await user.tab();
    expect(
      within(drawer).getByRole('button', { name: 'Close sessions' }),
    ).toHaveFocus();
    await user.tab();
    expect(search).toHaveFocus();
    await user.tab();
    expect(newChat).toHaveFocus();
    await user.tab({ shift: true });
    expect(search).toHaveFocus();
    await user.keyboard('{Escape}');
    expect(
      screen.queryByRole('dialog', { name: 'Sessions' }),
    ).not.toBeInTheDocument();
    expect(opener).toHaveFocus();
  });

  it('keeps the sessions drawer modal after a row action and lets row controls handle their own Escape', async () => {
    mobile();
    const user = userEvent.setup();
    const onArchive = vi.fn().mockResolvedValue(undefined);
    render(
      <Shell
        sidebar={
          <SessionSidebar
            sessions={[
              sessionFixture('chat:trip', {
                title: 'Trip ideas',
                lastActivityAtMs: Date.now(),
              }),
            ]}
            activeKey={null}
            query=""
            onQueryChange={vi.fn()}
            showArchived={false}
            onShowArchivedChange={vi.fn()}
            onOpen={vi.fn()}
            onRename={vi.fn().mockResolvedValue(true)}
            onArchive={onArchive}
            onExport={vi.fn().mockResolvedValue(undefined)}
            onDelete={vi.fn().mockResolvedValue(undefined)}
          />
        }
      />,
    );
    const opener = screen.getByRole('button', { name: 'Open sessions' });
    await user.click(opener);
    const drawer = screen.getByRole('dialog', { name: 'Sessions' });
    // Everything behind the drawer is out of reach while it is open.
    expect(opener.closest('[inert]')).not.toBeNull();
    expect(screen.queryByRole('banner')).not.toBeInTheDocument();

    // Archive closes its menu and returns focus to its own trigger (D5),
    // rather than falling through to the panel's generic body-focus rescue.
    await user.click(
      within(drawer).getByRole('button', { name: 'Actions for Trip ideas' }),
    );
    await user.click(within(drawer).getByRole('menuitem', { name: 'Archive' }));
    expect(onArchive).toHaveBeenCalledOnce();
    expect(
      within(drawer).getByRole('button', { name: 'Actions for Trip ideas' }),
    ).toHaveFocus();
    await user.tab();
    expect(drawer).toContainElement(document.activeElement as HTMLElement);
    await user.keyboard('{Escape}');
    expect(
      screen.queryByRole('dialog', { name: 'Sessions' }),
    ).not.toBeInTheDocument();
    expect(opener).toHaveFocus();
    expect(opener.closest('[inert]')).toBeNull();

    // Escape in a row menu or the rename field closes only that.
    await user.click(opener);
    const reopened = screen.getByRole('dialog', { name: 'Sessions' });
    await user.click(
      within(reopened).getByRole('button', { name: 'Actions for Trip ideas' }),
    );
    await user.keyboard('{Escape}');
    expect(within(reopened).queryByRole('menu')).not.toBeInTheDocument();
    expect(screen.getByRole('dialog', { name: 'Sessions' })).toBe(reopened);
    await user.click(
      within(reopened).getByRole('button', { name: 'Actions for Trip ideas' }),
    );
    await user.click(within(reopened).getByRole('menuitem', { name: 'Rename' }));
    expect(
      within(reopened).getByRole('textbox', { name: 'Rename Trip ideas' }),
    ).toHaveFocus();
    await user.keyboard('{Escape}');
    expect(
      within(reopened).queryByRole('textbox', { name: 'Rename Trip ideas' }),
    ).not.toBeInTheDocument();
    expect(screen.getByRole('dialog', { name: 'Sessions' })).toBe(reopened);
  });

  it('shows working helpers as status without introducing another persona', () => {
    const main = agent('agent-main', 'Nova', 1);
    render(
      <Shell
        mainAgent={main}
        agents={[agent('helper', 'Research', 2, { status: 'Running' }), main]}
      />,
    );
    expect(screen.getByText('1 helper is working')).toBeVisible();
    expect(
      screen.queryByRole('article', { name: 'Research agent' }),
    ).not.toBeInTheDocument();
  });

  it('exposes settings as a contextual action for the main agent', async () => {
    const onOpenSettings = vi.fn();
    render(<Shell onOpenSettings={onOpenSettings} />);
    const settings = screen.getByRole('button', { name: 'Settings' });
    expect(settings).toHaveAttribute('title', 'Settings for Nova');
    await userEvent.click(settings);
    expect(onOpenSettings).toHaveBeenCalledOnce();
  });

  it('places mobile navigation after workspace content in DOM and tab order', () => {
    mobile();
    render(<Shell conversation={<button type="button">Workspace action</button>} />);
    const content = screen.getByRole('main');
    const navigation = screen.getByRole('navigation', {
      name: 'Workspace navigation',
    });
    expect(navigation).toHaveAttribute('data-placement', 'bottom-dock');
    expect(content.parentElement?.nextElementSibling).toBe(navigation);
    expect(
      within(navigation).getByRole('button', { name: 'Chats' }),
    ).toHaveAttribute('aria-current', 'page');
    expect(
      screen
        .getByRole('button', { name: 'Workspace action' })
        .compareDocumentPosition(
          within(navigation).getByRole('button', { name: 'Chats' }),
        ) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).not.toBe(0);
  });

  it('shows the configured workspace company name next to the shell brand', () => {
    render(<Shell workspaceState={configuredWorkspace(false)} />);
    const sidebar = screen.getByRole('complementary');
    expect(within(sidebar).getByText('Welcome back')).toBeVisible();
    expect(within(sidebar).getByText('Northwind Research')).toBeVisible();
  });

  it('renders the presence block exactly as today when no workspace is configured', () => {
    render(<Shell workspaceState={null} />);
    const sidebar = screen.getByRole('complementary');
    expect(within(sidebar).getByText('Welcome back')).toBeVisible();
    expect(within(sidebar).getByRole('heading', { name: 'Nova' })).toBeVisible();
    expect(
      within(sidebar).queryByText('Northwind Research'),
    ).not.toBeInTheDocument();
  });

  it('hides the company name when the workspace state is not configured', () => {
    render(
      <Shell
        workspaceState={{
          configured: false,
          workspace: null,
          defaultRoot: '/workspaces',
        }}
      />,
    );
    const sidebar = screen.getByRole('complementary');
    expect(within(sidebar).getByText('Welcome back')).toBeVisible();
    expect(
      within(sidebar).queryByText('Northwind Research'),
    ).not.toBeInTheDocument();
  });

  it('opens Connectors as a page', async () => {
    render(<Shell connectors={<div>Manage connections</div>} />);
    await userEvent.click(screen.getByRole('button', { name: 'Connectors' }));
    expect(screen.getByText('Manage connections')).toBeVisible();
    expect(
      screen.queryByRole('button', { name: 'Telegram' }),
    ).not.toBeInTheDocument();
  });
});
