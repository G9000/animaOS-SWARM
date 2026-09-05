import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { daemon } from '../lib/daemon-api';
import type { AgentDetail } from '../lib/types';
import { WorkspaceHub } from './WorkspaceHub';

const agents: AgentDetail[] = ['Alpha', 'Beta'].map((name) => ({
  id: name.toLowerCase(),
  name,
  provider: 'test',
  model: 'test',
  status: 'Idle',
  toolNames: [],
  messages: [],
  created_at_ms: 0,
  token_usage: { prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 },
}));
afterEach(() => vi.restoreAllMocks());

it('keeps the editor open during a save and retains a failed draft', async () => {
  vi.spyOn(daemon, 'recentAgentMemories').mockResolvedValue({ memories: [] });
  vi.spyOn(daemon, 'agentTasks').mockResolvedValue({
    tasks: [],
    revision: '1',
  });
  let rejectSave!: (reason: Error) => void;
  vi.spyOn(daemon, 'updateAgentTasks').mockImplementation(
    () =>
      new Promise((_resolve, reject) => {
        rejectSave = reject;
      }),
  );
  render(<WorkspaceHub agents={[agents[0]]} />);
  fireEvent.click(screen.getByRole('tab', { name: 'Tasks' }));
  fireEvent.click(screen.getByRole('button', { name: 'Manage Alpha’s tasks' }));
  fireEvent.change(await screen.findByLabelText('New agent task'), {
    target: { value: 'Draft task' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Add task' }));
  fireEvent.click(screen.getByRole('button', { name: 'Save tasks' }));
  expect(screen.getByRole('button', { name: /Back to tasks/ })).toBeDisabled();
  rejectSave(new Error('Save failed'));
  expect(await screen.findByRole('alert')).toHaveTextContent('Save failed');
  expect(screen.getByLabelText('Task 1')).toHaveValue('Draft task');
  expect(screen.getByRole('button', { name: /Back to tasks/ })).toBeEnabled();
});

it('aggregates notes, filters by owner, and opens the owning chat without refetching on status polls', async () => {
  const notes = vi
    .spyOn(daemon, 'recentAgentMemories')
    .mockImplementation(async (id) => ({
      memories: [
        {
          id: 'same-id',
          agentId: id,
          agentName: id,
          content: `${id} saved note`,
          type: 'fact',
          scope: 'private',
          importance: 1,
          createdAt: '2026-09-06T00:00:00Z',
        },
      ],
    }));
  const onSelectAgent = vi.fn();
  const { rerender } = render(
    <WorkspaceHub agents={agents} onSelectAgent={onSelectAgent} />,
  );
  expect(await screen.findByText('alpha saved note')).toBeVisible();
  expect(screen.getByText('beta saved note')).toBeVisible();
  rerender(
    <WorkspaceHub
      agents={agents.map((agent) => ({ ...agent, status: 'Running' }))}
      onSelectAgent={onSelectAgent}
    />,
  );
  expect(notes).toHaveBeenCalledTimes(2);
  fireEvent.change(screen.getByLabelText('Agent'), {
    target: { value: 'beta' },
  });
  expect(screen.queryByText('alpha saved note')).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Chat with Beta' }));
  expect(onSelectAgent).toHaveBeenCalledWith('beta');
  fireEvent.change(screen.getByLabelText('Search'), {
    target: { value: 'absent' },
  });
  expect(screen.getByText('No matches. Try another search.')).toBeVisible();
});

it('keeps successful agent data when another fails and retries explicitly', async () => {
  vi.spyOn(daemon, 'recentAgentMemories').mockResolvedValue({ memories: [] });
  const tasks = vi
    .spyOn(daemon, 'agentTasks')
    .mockImplementation(async (id) => {
      if (id === 'beta') throw new Error('Unavailable');
      return {
        revision: '1',
        tasks: [
          {
            content: 'Write campaign',
            status: 'pending',
            activeForm: 'Writing',
          },
        ],
      };
    });
  render(<WorkspaceHub agents={agents} />);
  fireEvent.click(screen.getByRole('tab', { name: 'Tasks' }));
  expect(await screen.findByText('Write campaign')).toBeVisible();
  expect(screen.getByRole('alert')).toHaveTextContent('Beta: Unavailable');
  tasks.mockResolvedValue({ revision: '2', tasks: [] });
  fireEvent.click(screen.getByRole('button', { name: 'Refresh' }));
  await waitFor(() =>
    expect(screen.queryByRole('alert')).not.toBeInTheDocument(),
  );
  expect(await screen.findByText('No tasks yet.')).toBeVisible();
});

it('ignores a stale notes response after switching to schedules', async () => {
  let resolveNotes!: (value: { memories: [] }) => void;
  vi.spyOn(daemon, 'recentAgentMemories').mockImplementation(
    () =>
      new Promise((resolve) => {
        resolveNotes = resolve;
      }),
  );
  vi.spyOn(daemon, 'listSchedules').mockResolvedValue({ schedules: [] });
  render(<WorkspaceHub agents={[agents[0]]} />);
  fireEvent.click(screen.getByRole('tab', { name: 'Schedules' }));
  expect(await screen.findByText('No schedules yet.')).toBeVisible();
  resolveNotes({ memories: [] });
  await waitFor(() =>
    expect(screen.getByText('No schedules yet.')).toBeVisible(),
  );
  expect(
    screen.getByRole('button', { name: 'Manage Alpha’s schedules' }),
  ).toBeVisible();
});
