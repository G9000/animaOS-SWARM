import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import type { AgentDetail } from '../lib/types';
import { WorkspaceShell } from './WorkspaceShell';

const companion: AgentDetail = {
  id: 'main',
  name: 'Anima',
  provider: 'openai',
  model: 'test-model',
  status: 'Idle',
  created_at_ms: 1,
  toolNames: [],
  messages: [],
  token_usage: { prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 },
};

describe('single companion experience', () => {
  it('opens the conversation immediately without swarm management or agent switching', () => {
    render(
      <WorkspaceShell
        mainAgent={companion}
        agents={[
          companion,
          { ...companion, id: 'helper', name: 'Research helper' },
        ]}
        connection="online"
        workspace={<div>My conversation</div>}
        activity={<div>Activity content</div>}
        onOpenSettings={vi.fn()}
      />,
    );
    expect(screen.getByText('My conversation')).toBeVisible();
    expect(
      screen.getByRole('button', { name: 'Chat', exact: true }),
    ).toHaveAttribute('aria-current', 'page');
    for (const name of ['Team', 'Operations', 'Overview']) {
      expect(
        screen.queryByRole('button', { name, exact: true }),
      ).not.toBeInTheDocument();
    }
    expect(
      screen.queryByRole('combobox', { name: 'Chat with agent' }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole('navigation', { name: 'Direct messages' }),
    ).not.toBeInTheDocument();
    expect(
      within(screen.getByRole('complementary')).getByRole('heading', {
        name: 'Anima',
      }),
    ).toBeVisible();
  });

  it('keeps the conversation mounted when checking another page', async () => {
    render(
      <WorkspaceShell
        mainAgent={companion}
        agents={[companion]}
        connection="online"
        workspace={
          <input aria-label="Unsaved draft" defaultValue="Remember this" />
        }
        activity={<div>Activity content</div>}
        onOpenSettings={vi.fn()}
      />,
    );
    const input = screen.getByLabelText('Unsaved draft');
    await userEvent.click(
      screen.getByRole('button', { name: 'Activity', exact: true }),
    );
    expect(screen.getByText('Activity content')).toBeVisible();
    expect(input).not.toBeVisible();
    await userEvent.click(
      screen.getByRole('button', { name: 'Chat', exact: true }),
    );
    expect(screen.getByLabelText('Unsaved draft')).toBe(input);
    expect(input).toHaveValue('Remember this');
  });

  it('describes disconnection without promising work is still running', () => {
    render(
      <WorkspaceShell
        mainAgent={companion}
        agents={[companion]}
        connection="offline"
        workspace={<div>My conversation</div>}
        activity={null}
        onOpenSettings={vi.fn()}
      />,
    );
    expect(screen.getByText('Offline')).toBeVisible();
    expect(screen.getByText('Cannot reach your companion')).toBeVisible();
  });
});
