import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';

import { toolNamesForProfile } from '../lib/agent-access';
import type { AgentDetail } from '../lib/types';
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

describe('WorkspaceShell', () => {
  it('owns one responsive navigation with Workspace, Activity, and Agents destinations', async () => {
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
    expect(navigation).toHaveAttribute('data-desktop-placement', 'top');
    expect(navigation).toHaveAttribute('data-mobile-placement', 'bottom-dock');
    expect(
      within(navigation).getByRole('button', { name: 'Workspace' }),
    ).toHaveAttribute('aria-current', 'page');
    expect(
      within(navigation).getByRole('button', { name: 'Activity' }),
    ).toBeVisible();
    expect(
      within(navigation).getByRole('button', { name: 'Agents' }),
    ).toBeVisible();
    expect(screen.getByText('Workspace canvas')).toBeVisible();

    await user.click(
      within(navigation).getByRole('button', { name: 'Activity' }),
    );
    expect(screen.getByText('Activity canvas')).toBeVisible();
    expect(screen.queryByText('Workspace canvas')).not.toBeInTheDocument();

    await user.click(
      within(navigation).getByRole('button', { name: 'Agents' }),
    );
    expect(screen.getByRole('heading', { name: 'Agents' })).toBeVisible();
    expect(
      within(screen.getByRole('article', { name: 'Nova agent' })).getByText(
        'Main',
      ),
    ).toBeVisible();
  });

  it('labels daemon, agent, and access status with text and icons', () => {
    const nova = agent('agent-main', 'Nova', 1, {
      status: 'Running',
      toolNames: ['read_file', 'bespoke_tool'],
    });

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

    expect(screen.getByText('Daemon Offline')).toBeVisible();
    expect(screen.getByLabelText('Daemon offline')).toBeVisible();
    expect(screen.getByText('Agent Running')).toBeVisible();
    expect(screen.getByLabelText('Agent running')).toBeVisible();
    expect(screen.getByText('Access Custom')).toBeVisible();
    expect(screen.getByLabelText('Custom access profile')).toBeVisible();
  });

  it('marks the selected agent as Main and keeps additional agents read-only', async () => {
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

    await user.click(screen.getByRole('button', { name: 'Agents' }));
    const mainEntry = screen.getByRole('article', { name: 'Nova agent' });
    const additionalEntry = screen.getByRole('article', { name: 'Echo agent' });
    expect(within(mainEntry).getByText('Main')).toBeVisible();
    expect(within(additionalEntry).getByText('Read only')).toBeVisible();
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
});
