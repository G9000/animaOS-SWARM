import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { toolNamesForProfile } from '../lib/agent-access';
import type { AgentDetail } from '../lib/types';
import { WorkspaceShell } from './WorkspaceShell';
import { daemon } from '../lib/daemon-api';

it('lands on the overview and opens the manager conversation deliberately', async () => {
  vi.spyOn(daemon, 'agentTasks').mockResolvedValue({ tasks: [], revision: '1' });
  vi.spyOn(daemon, 'listSchedules').mockResolvedValue({ schedules: [] });
  const main = agent('main', 'Nova', 1);
  const select = vi.fn();
  render(<WorkspaceShell mainAgent={main} agents={[main]} connection="online"
    workspace={<div>Conversation canvas</div>} activity={<div>Activity</div>}
    onSelectAgent={select} onOpenSettings={vi.fn()} />);
  expect(screen.getByRole('button', { name: 'Overview', exact: true })).toHaveAttribute('aria-current', 'page');
  expect(screen.queryByText('Conversation canvas')).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: 'Ask your manager' }));
  expect(select).toHaveBeenCalledWith('main');
  expect(screen.getByText('Conversation canvas')).toBeVisible();
});

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

beforeEach(() => {
  vi.spyOn(daemon, 'agentTasks').mockResolvedValue({ tasks: [], revision: '1' });
  vi.spyOn(daemon, 'listSchedules').mockResolvedValue({ schedules: [] });
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('WorkspaceShell', () => {
  it('opens capabilities from workplace navigation', async () => {
    vi.spyOn(daemon, 'capabilities').mockResolvedValue({ schemaVersion: 1, tools: [], persistence: { controlPlane: 'file', memory: 'file', executionJournal: false }, extensions: [], limitations: [] });
    const nova = agent('agent-main', 'Nova', 1);
    render(<WorkspaceShell mainAgent={nova} agents={[nova]} connection="online" workspace={<div>Conversation</div>} activity={<div>Activity</div>} onOpenSettings={vi.fn()} />);
    await userEvent.click(screen.getByRole('button', { name: 'Capabilities', exact: true }));
    expect(await screen.findByText('Tools follow your authority')).toBeVisible();
    expect(screen.getByRole('button', { name: 'Capabilities', exact: true })).toHaveAttribute('aria-current', 'page');
  });
  it('opens commands with Control K, filters actions and navigates with Enter', async () => {
    const user = userEvent.setup();
    const nova = agent('agent-main', 'Nova', 1);
    render(
      <WorkspaceShell
        mainAgent={nova}
        agents={[nova]}
        connection="online"
        workspace={<div>Workspace canvas</div>}
        activity={<div>Activity canvas</div>}
        onOpenSettings={vi.fn()}
      />,
    );
    await user.keyboard('{Control>}k{/Control}');
    expect(screen.getByRole('dialog', { name: 'Command menu' })).toBeVisible();
    await user.type(
      screen.getByRole('combobox', { name: 'Search commands' }),
      'activity',
    );
    await user.keyboard('{Enter}');
    expect(screen.getByText('Activity canvas')).toBeVisible();
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('can enter and leave focus mode without losing the workspace', async () => {
    const user = userEvent.setup();
    const nova = agent('agent-main', 'Nova', 1);
    render(
      <WorkspaceShell
        mainAgent={nova}
        agents={[nova]}
        connection="online"
        workspace={<div>Workspace canvas</div>}
        activity={<div>Activity canvas</div>}
        onOpenSettings={vi.fn()}
      />,
    );
    await user.click(screen.getByRole('button', { name: 'Enter focus mode' }));
    expect(screen.queryByRole('complementary')).not.toBeInTheDocument();
    expect(screen.getByRole('heading', { name: 'Overview', exact: true })).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Exit focus mode' }));
    expect(screen.getByRole('complementary')).toBeVisible();
  });

  it('contains keyboard focus in commands and restores the opener on Escape', async () => {
    const user = userEvent.setup();
    const nova = agent('agent-main', 'Nova', 1);
    render(
      <WorkspaceShell
        mainAgent={nova}
        agents={[nova]}
        connection="online"
        workspace={<div>Workspace canvas</div>}
        activity={<div>Activity canvas</div>}
        onOpenSettings={vi.fn()}
      />,
    );
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
    const nova = agent('agent-main', 'Nova', 1);
    const pick = vi.fn();
    render(
      <WorkspaceShell
        mainAgent={nova}
        agents={[nova]}
        connection="online"
        workspace={<div>Workspace canvas</div>}
        activity={<div>Activity canvas</div>}
        onOpenSettings={vi.fn()}
        onPickPrompt={pick}
      />,
    );
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

  it('filters agents by provider and shows a useful empty state', async () => {
    const user = userEvent.setup();
    const nova = agent('agent-main', 'Nova', 1);
    const scout = agent('scout', 'Scout', 2, { provider: 'anthropic' });
    render(
      <WorkspaceShell
        mainAgent={nova}
        agents={[nova, scout]}
        connection="online"
        workspace={<div>Workspace canvas</div>}
        activity={<div>Activity canvas</div>}
        onOpenSettings={vi.fn()}
      />,
    );
    await user.click(screen.getByRole('button', { name: 'Team' }));
    await user.type(
      screen.getByRole('searchbox', { name: 'Search agents' }),
      'anthropic',
    );
    expect(screen.getByRole('article', { name: 'Scout agent' })).toBeVisible();
    expect(
      screen.queryByRole('article', { name: 'Nova agent' }),
    ).not.toBeInTheDocument();
    await user.type(
      screen.getByRole('searchbox', { name: 'Search agents' }),
      'missing',
    );
    expect(screen.getByText('No agents match your search.')).toBeVisible();
  });
  it('shows a dedicated Telegram destination only when a connector exists', async () => {
    const user = userEvent.setup();
    const nova = agent('agent-main', 'Nova', 1);
    const view = render(
      <WorkspaceShell
        mainAgent={nova}
        agents={[nova]}
        connection="online"
        workspace={<div>Workspace canvas</div>}
        activity={<div>Activity canvas</div>}
        telegram={null}
        onOpenSettings={vi.fn()}
      />,
    );
    expect(
      screen.queryByRole('button', { name: 'Telegram' }),
    ).not.toBeInTheDocument();
    view.rerender(
      <WorkspaceShell
        mainAgent={nova}
        agents={[nova]}
        connection="online"
        workspace={<div>Workspace canvas</div>}
        activity={<div>Activity canvas</div>}
        telegram={<div>Telegram canvas</div>}
        onOpenSettings={vi.fn()}
      />,
    );
    await user.click(screen.getByRole('button', { name: 'Telegram' }));
    expect(screen.getByText('Telegram canvas')).toBeVisible();
  });

  it('uses a left sidebar for desktop workspace navigation', async () => {
    const user = userEvent.setup();
    const nova = agent('agent-main', 'Nova', 1);

    render(
      <WorkspaceShell
        mainAgent={nova}
        agents={[nova]}
        connection="online"
        workspace={<div>Workspace canvas</div>}
        activity={<div>Activity canvas</div>}
        onOpenSettings={vi.fn()}
      />,
    );

    const navigation = screen.getByRole('navigation', {
      name: 'Workspace navigation',
    });
    expect(navigation).toHaveAttribute('data-placement', 'sidebar');
    expect(navigation).toHaveAttribute('aria-orientation', 'vertical');
    const sidebar = navigation.closest('aside');
    expect(sidebar).not.toBeNull();
    expect(sidebar?.nextElementSibling?.tagName).toBe('MAIN');
    expect(
      within(navigation).getByRole('button', { name: 'Overview' }),
    ).toHaveAttribute('aria-current', 'page');
    expect(
      within(navigation).getByRole('button', { name: 'Activity' }),
    ).toBeVisible();
    expect(
      within(navigation).getByRole('button', { name: 'Team' }),
    ).toBeVisible();
    expect(screen.getByRole('heading', { name: 'Overview', exact: true })).toBeVisible();

    await user.click(
      within(navigation).getByRole('button', { name: 'Activity' }),
    );
    expect(screen.getByText('Activity canvas')).toBeVisible();
    expect(screen.queryByText('Workspace canvas')).not.toBeInTheDocument();

    await user.click(
      within(navigation).getByRole('button', { name: 'Team' }),
    );
    expect(screen.getByRole('heading', { name: 'Team' })).toBeVisible();
    expect(
      within(screen.getByRole('article', { name: 'Nova agent' })).getByText(
        'Main',
      ),
    ).toBeVisible();
  });

  it('shows the main agent identity in the sidebar presence block', () => {
    const nova = agent('agent-main', 'Nova', 1);

    render(
      <WorkspaceShell
        mainAgent={nova}
        agents={[nova]}
        connection="offline"
        workspace={<div>Workspace canvas</div>}
        activity={<div>Activity canvas</div>}
        onOpenSettings={vi.fn()}
      />,
    );

    const sidebar = screen.getByRole('complementary');
    expect(
      within(sidebar).getByRole('heading', { name: 'Nova' }),
    ).toBeVisible();
    expect(within(sidebar).getByText('Welcome back')).toBeVisible();
    expect(within(sidebar).getByText('Main')).toBeVisible();
  });

  it('shows the persisted workspace avatar and uploads a replacement', async () => {
    const user = userEvent.setup();
    const nova = agent('agent-main', 'Nova', 1);
    const onChangeWorkspaceAvatar = vi.fn().mockResolvedValue(undefined);
    Object.defineProperties(URL, {
      createObjectURL: {
        configurable: true,
        value: vi.fn(() => 'blob:workspace-avatar-preview'),
      },
      revokeObjectURL: {
        configurable: true,
        value: vi.fn(),
      },
    });

    render(
      <WorkspaceShell
        mainAgent={nova}
        agents={[nova]}
        connection="online"
        workspaceState={{
          configured: true,
          workspace: {
            rootPath: '/workspaces/northwind',
            companyName: 'Northwind Research',
            mission: 'Map supply chains',
            values: ['rigor'],
            hasAvatar: true,
          },
          defaultRoot: '/workspaces',
        }}
        workspace={<div>Workspace canvas</div>}
        activity={<div>Activity canvas</div>}
        onOpenSettings={vi.fn()}
        onChangeWorkspaceAvatar={onChangeWorkspaceAvatar}
      />,
    );

    expect(
      screen
        .getByRole('button', { name: 'Change workspace avatar' })
        .querySelector('img'),
    ).toHaveAttribute('src', '/api/workspace/avatar?v=0');

    const file = new File(['avatar'], 'avatar.png', { type: 'image/png' });
    await user.upload(
      screen.getByLabelText('Workspace avatar image file'),
      file,
    );

    await waitFor(() =>
      expect(onChangeWorkspaceAvatar).toHaveBeenCalledWith(file),
    );
  });

  it('shows a compact presence bar on mobile', () => {
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
    const nova = agent('agent-main', 'Nova', 1, { status: 'Running' });

    render(
      <WorkspaceShell
        mainAgent={nova}
        agents={[nova]}
        connection="offline"
        workspace={<div>Workspace canvas</div>}
        activity={<div>Activity canvas</div>}
        onOpenSettings={vi.fn()}
      />,
    );

    const bar = screen.getByRole('banner');
    expect(within(bar).getByRole('heading', { name: 'Nova' })).toBeVisible();
    expect(within(bar).getByRole('button', { name: 'Settings' })).toBeVisible();
  });

  it('marks the main agent and identifies additional teammates', async () => {
    const user = userEvent.setup();
    const main = agent('agent-main', 'Nova', 1);
    const additional = agent('agent-other', 'Echo', 2, { status: 'Completed' });

    render(
      <WorkspaceShell
        mainAgent={main}
        agents={[additional, main]}
        connection="online"
        workspace={<div>Workspace canvas</div>}
        activity={<div>Activity canvas</div>}
        onOpenSettings={vi.fn()}
      />,
    );

    await user.click(screen.getByRole('button', { name: 'Team' }));
    const mainEntry = screen.getByRole('article', { name: 'Nova agent' });
    const additionalEntry = screen.getByRole('article', { name: 'Echo agent' });
    expect(within(mainEntry).getByText('Main')).toBeVisible();
    expect(within(additionalEntry).getByText('Teammate')).toBeVisible();
    expect(within(additionalEntry).getByText('Agent Completed')).toBeVisible();
    expect(
      within(additionalEntry).queryByRole('button'),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole('button', { name: /create agent/i }),
    ).not.toBeInTheDocument();
  });

  it('exposes settings as a contextual action for the main agent', async () => {
    const user = userEvent.setup();
    const onOpenSettings = vi.fn();
    const nova = agent('agent-main', 'Nova', 1);

    render(
      <WorkspaceShell
        mainAgent={nova}
        agents={[nova]}
        connection="online"
        workspace={<div>Workspace canvas</div>}
        activity={<div>Activity canvas</div>}
        onOpenSettings={onOpenSettings}
      />,
    );

    const settings = screen.getByRole('button', { name: 'Settings' });
    expect(settings).toHaveAttribute('title', 'Settings for Nova');
    await user.click(settings);
    expect(onOpenSettings).toHaveBeenCalledOnce();
  });

  it('places mobile navigation after workspace content in DOM and tab order', () => {
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
    const nova = agent('agent-main', 'Nova', 1);

    render(
      <WorkspaceShell
        mainAgent={nova}
        agents={[nova]}
        connection="online"
        workspace={<button type="button">Workspace action</button>}
        activity={<div>Activity canvas</div>}
        onOpenSettings={vi.fn()}
      />,
    );

    const content = screen.getByRole('main');
    const navigation = screen.getByRole('navigation', {
      name: 'Workspace navigation',
    });
    expect(navigation).toHaveAttribute('data-placement', 'bottom-dock');
    expect(content.parentElement?.nextElementSibling).toBe(navigation);
    expect(
      screen
        .getByRole('button', { name: 'Ask your manager' })
        .compareDocumentPosition(
          screen.getByRole('button', { name: 'Overview' }),
        ) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).not.toBe(0);
  });

  it('shows the configured workspace company name next to the shell brand', () => {
    const nova = agent('agent-main', 'Nova', 1);

    render(
      <WorkspaceShell
        mainAgent={nova}
        agents={[nova]}
        connection="online"
        workspaceState={{
          configured: true,
          workspace: {
            rootPath: '/workspaces/northwind',
            companyName: 'Northwind Research',
            mission: 'Map supply chains',
            values: ['rigor'],
            hasAvatar: false,
          },
          defaultRoot: '/workspaces',
        }}
        workspace={<div>Workspace canvas</div>}
        activity={<div>Activity canvas</div>}
        onOpenSettings={vi.fn()}
      />,
    );

    const sidebar = screen.getByRole('complementary');
    expect(within(sidebar).getByText('Welcome back')).toBeVisible();
    expect(within(sidebar).getByText('Northwind Research')).toBeVisible();
  });

  it('renders the presence block exactly as today when no workspace is configured', () => {
    const nova = agent('agent-main', 'Nova', 1);

    render(
      <WorkspaceShell
        mainAgent={nova}
        agents={[nova]}
        connection="online"
        workspaceState={null}
        workspace={<div>Workspace canvas</div>}
        activity={<div>Activity canvas</div>}
        onOpenSettings={vi.fn()}
      />,
    );

    const sidebar = screen.getByRole('complementary');
    expect(within(sidebar).getByText('Welcome back')).toBeVisible();
    expect(
      within(sidebar).getByRole('heading', { name: 'Nova' }),
    ).toBeVisible();
    expect(
      within(sidebar).queryByText('Northwind Research'),
    ).not.toBeInTheDocument();
  });

  it('hides the company name when the workspace state is not configured', () => {
    const nova = agent('agent-main', 'Nova', 1);

    render(
      <WorkspaceShell
        mainAgent={nova}
        agents={[nova]}
        connection="online"
        workspaceState={{
          configured: false,
          workspace: null,
          defaultRoot: '/workspaces',
        }}
        workspace={<div>Workspace canvas</div>}
        activity={<div>Activity canvas</div>}
        onOpenSettings={vi.fn()}
      />,
    );

    const sidebar = screen.getByRole('complementary');
    expect(within(sidebar).getByText('Welcome back')).toBeVisible();
    expect(
      within(sidebar).queryByText('Northwind Research'),
    ).not.toBeInTheDocument();
  });
});

it('opens Connectors independently of the Telegram conversation', async () => {
  const nova = agent('main', 'Nova', 1);
  render(
    <WorkspaceShell
      mainAgent={nova}
      agents={[nova]}
      connection="online"
      workspace={<div>Workspace</div>}
      activity={<div>Activity</div>}
      connectors={<div>Manage connections</div>}
      telegram={<div>Telegram conversation</div>}
      onOpenSettings={vi.fn()}
    />,
  );
  await userEvent.click(screen.getByRole('button', { name: 'Connectors' }));
  expect(screen.getByText('Manage connections')).toBeVisible();
  expect(screen.queryByText('Telegram conversation')).not.toBeInTheDocument();
});
