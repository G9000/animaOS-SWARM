import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { daemon, type DaemonSchedule } from '../lib/daemon-api';
import type { AgentDetail } from '../lib/types';
import { WorkspaceDashboard } from './WorkspaceDashboard';

const agents: AgentDetail[] = [
  {
    id: 'main',
    name: 'Anima',
    provider: 'chatgpt',
    model: 'gpt-5.5',
    status: 'Running',
    created_at_ms: 0,
    toolNames: [],
    token_usage: { prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 },
    messages: [
      {
        id: 'reply',
        role: 'Assistant',
        content: { text: 'The draft is ready for review.' },
        created_at_ms: 10,
      },
    ],
  },
];
const schedule: DaemonSchedule = {
  id: 'schedule',
  agentId: 'main',
  prompt: 'Review the campaign',
  enabled: true,
  trigger: { type: 'interval', intervalMs: 3600000 },
  target: { type: 'workspace' },
  nextDueAtMs: Date.now() + 3600000,
  importIdempotencyKey: null,
  lastOutcome: {
    status: 'error',
    occurredAtMs: 0,
    errorCode: 'provider_error',
  },
};
const callbacks = () => ({
  onChat: vi.fn(),
  onOpenWork: vi.fn(),
  onOpenTeam: vi.fn(),
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

it('combines live work and schedules and routes actions to their owner', async () => {
  const tasks = vi.spyOn(daemon, 'agentTasks').mockResolvedValue({
    revision: '1',
    tasks: [
      {
        content: 'Draft guidelines',
        status: 'in_progress',
        activeForm: 'Drafting',
      },
      {
        content: 'Research complete',
        status: 'completed',
        activeForm: 'Researching',
      },
    ],
  });
  vi.spyOn(daemon, 'listSchedules').mockResolvedValue({
    schedules: [schedule],
  });
  const actions = callbacks();
  const { rerender } = render(
    <WorkspaceDashboard agents={agents} online {...actions} />,
  );
  expect(await screen.findByText('Draft guidelines')).toBeVisible();
  expect(screen.getByText('Research complete')).toBeVisible();
  expect(
    screen.getByText('Completed task · completion time unavailable'),
  ).toBeVisible();
  expect(screen.getByText('Review the campaign')).toBeVisible();
  expect(
    within(
      screen.getByRole('region', { name: 'Needs your attention' }),
    ).getByRole('button'),
  ).toHaveTextContent('Schedule needs review');
  fireEvent.click(screen.getByRole('button', { name: 'View tasks' }));
  expect(actions.onOpenWork).toHaveBeenCalledWith('Tasks');
  fireEvent.click(screen.getByRole('button', { name: 'View schedules' }));
  expect(actions.onOpenWork).toHaveBeenCalledWith('Schedules');
  fireEvent.click(screen.getByRole('button', { name: /The draft is ready/ }));
  expect(actions.onChat).toHaveBeenCalledWith('main');
  rerender(
    <WorkspaceDashboard
      agents={agents.map((agent) => ({ ...agent, status: 'Completed' }))}
      online
      {...actions}
    />,
  );
  expect(tasks).toHaveBeenCalledOnce();
});

it('preserves available schedules when tasks fail and retries', async () => {
  const tasks = vi
    .spyOn(daemon, 'agentTasks')
    .mockRejectedValue(new Error('offline'));
  vi.spyOn(daemon, 'listSchedules').mockResolvedValue({
    schedules: [schedule],
  });
  render(<WorkspaceDashboard agents={agents} online {...callbacks()} />);
  expect(await screen.findByRole('alert')).toHaveTextContent(
    'Some task or schedule data could not be loaded',
  );
  expect(screen.getByText('Review the campaign')).toBeVisible();
  expect(
    screen.getByText('Tasks will appear when their data is available.'),
  ).toBeVisible();
  tasks.mockResolvedValue({ tasks: [], revision: '2' });
  fireEvent.click(screen.getByRole('button', { name: 'Refresh overview' }));
  await waitFor(() =>
    expect(screen.queryByRole('alert')).not.toBeInTheDocument(),
  );
  expect(await screen.findByText(/No open tasks/)).toBeVisible();
});

it('does not report a healthy workspace when offline', async () => {
  const tasks = vi.spyOn(daemon, 'agentTasks');
  render(
    <WorkspaceDashboard agents={agents} online={false} {...callbacks()} />,
  );
  expect(screen.getByText(/The daemon is offline/)).toBeVisible();
  expect(
    screen.getByRole('button', { name: 'Refresh overview' }),
  ).toBeDisabled();
  expect(tasks).not.toHaveBeenCalled();
  expect(
    screen.queryByText('No failed agent runs or schedule errors reported.'),
  ).not.toBeInTheDocument();
});

it('offers a prepared first assignment only on explicit click for a fresh, idle manager', async () => {
  vi.spyOn(daemon, 'agentTasks').mockResolvedValue({
    tasks: [],
    revision: '1',
  });
  vi.spyOn(daemon, 'listSchedules').mockResolvedValue({ schedules: [] });
  const manager: AgentDetail = {
    ...agents[0],
    status: 'Idle',
    messages: [],
    system:
      'Role\n\nPrepared first assignment (do not start until the owner asks):\nPrepare a delivery brief.\n\nTeam responsibilities:\nWriter: draft',
  };
  const actions = {
    ...callbacks(),
    onStartAssignment: vi.fn(),
    onOpenFiles: vi.fn(),
  };
  const { rerender } = render(
    <WorkspaceDashboard
      agents={[manager]}
      managerId="main"
      mission="Help customers"
      online
      {...actions}
    />,
  );
  expect(screen.getByRole('heading', { name: 'Overview' })).toBeVisible();
  expect(screen.getByText('Help customers')).toBeVisible();
  expect(actions.onStartAssignment).not.toHaveBeenCalled();
  fireEvent.click(
    screen.getByRole('button', { name: 'Start first assignment' }),
  );
  expect(actions.onStartAssignment).toHaveBeenCalledWith(
    'Prepare a delivery brief.',
  );
  fireEvent.click(screen.getByRole('button', { name: 'Ask your manager' }));
  expect(actions.onChat).toHaveBeenCalledWith('main');
  fireEvent.click(screen.getByRole('button', { name: 'Open files' }));
  expect(actions.onOpenFiles).toHaveBeenCalledOnce();
  rerender(
    <WorkspaceDashboard
      agents={[
        {
          ...manager,
          messages: [
            {
              id: 'user',
              role: 'User',
              content: { text: 'Start' },
              created_at_ms: 1,
            },
          ],
        },
      ]}
      managerId="main"
      online
      {...actions}
    />,
  );
  expect(
    screen.queryByRole('button', { name: 'Start first assignment' }),
  ).not.toBeInTheDocument();
});

it('does not start a prepared assignment while any team member runs or while offline', () => {
  const manager: AgentDetail = {
    ...agents[0],
    status: 'Idle',
    messages: [],
    system:
      'Prepared first assignment (do not start until the owner asks):\nPrepare a brief.',
  };
  const actions = { ...callbacks(), onStartAssignment: vi.fn() };
  vi.spyOn(daemon, 'agentTasks').mockResolvedValue({
    tasks: [],
    revision: '1',
  });
  vi.spyOn(daemon, 'listSchedules').mockResolvedValue({ schedules: [] });
  const { rerender } = render(
    <WorkspaceDashboard
      agents={[manager, { ...agents[0], id: 'worker' }]}
      managerId="main"
      online
      {...actions}
    />,
  );
  expect(
    screen.queryByRole('button', { name: 'Start first assignment' }),
  ).not.toBeInTheDocument();
  rerender(
    <WorkspaceDashboard
      agents={[manager]}
      managerId="main"
      online={false}
      {...actions}
    />,
  );
  expect(
    screen.queryByRole('button', { name: 'Start first assignment' }),
  ).not.toBeInTheDocument();
  expect(actions.onStartAssignment).not.toHaveBeenCalled();
});

it('refreshes current work every 30 seconds and stops on unmount', async () => {
  vi.useFakeTimers();
  const tasks = vi
    .spyOn(daemon, 'agentTasks')
    .mockResolvedValue({ tasks: [], revision: '1' });
  vi.spyOn(daemon, 'listSchedules').mockResolvedValue({ schedules: [] });
  const view = render(
    <WorkspaceDashboard agents={agents} online {...callbacks()} />,
  );
  await act(async () => {
    await vi.advanceTimersByTimeAsync(0);
  });
  expect(tasks).toHaveBeenCalledTimes(1);
  await act(async () => {
    await vi.advanceTimersByTimeAsync(30_000);
  });
  expect(tasks).toHaveBeenCalledTimes(2);
  view.unmount();
  await act(async () => {
    await vi.advanceTimersByTimeAsync(30_000);
  });
  expect(tasks).toHaveBeenCalledTimes(2);
});
