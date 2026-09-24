import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState, type ReactNode } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { daemon } from '../lib/daemon-api';
import type { HashRoute } from '../lib/hash-route';
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

function Shell({
  agents = [companion],
  connection = 'online',
  conversation,
}: {
  agents?: AgentDetail[];
  connection?: 'online' | 'offline';
  conversation: ReactNode;
}) {
  const [route, setRoute] = useState<HashRoute>({ kind: 'home' });
  return (
    <WorkspaceShell
      mainAgent={companion}
      agents={agents}
      connection={connection}
      route={route}
      navigate={(next) => setRoute(next)}
      conversation={conversation}
      onOpenSettings={vi.fn()}
    />
  );
}

beforeEach(() => {
  vi.spyOn(daemon, 'agentJobs').mockResolvedValue([]);
  vi.spyOn(daemon, 'agentTasks').mockResolvedValue({ tasks: [], revision: '1' });
  vi.spyOn(daemon, 'listSchedules').mockResolvedValue({ schedules: [] });
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('single companion experience', () => {
  it('opens the conversation immediately without swarm management or agent switching', () => {
    render(
      <Shell
        agents={[companion, { ...companion, id: 'helper', name: 'Research helper' }]}
        conversation={<div>My conversation</div>}
      />,
    );
    expect(screen.getByText('My conversation')).toBeVisible();
    expect(screen.getByRole('button', { name: 'New chat' })).toBeVisible();
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
      <Shell
        conversation={
          <input aria-label="Unsaved draft" defaultValue="Remember this" />
        }
      />,
    );
    const input = screen.getByLabelText('Unsaved draft');
    await userEvent.click(screen.getByRole('button', { name: 'Work', exact: true }));
    expect(input).not.toBeVisible();
    await userEvent.click(
      screen.getByRole('button', { name: 'Open companion chat' }),
    );
    expect(screen.getByLabelText('Unsaved draft')).toBe(input);
    expect(input).toHaveValue('Remember this');
  });

  it('describes disconnection without promising work is still running', () => {
    render(<Shell connection="offline" conversation={<div>My conversation</div>} />);
    expect(screen.getByText('Offline')).toBeVisible();
    expect(screen.getByText('Cannot reach your companion')).toBeVisible();
  });
});
