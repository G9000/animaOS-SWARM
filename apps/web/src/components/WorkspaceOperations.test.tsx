import { fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { AgentJob } from '@animaOS-SWARM/sdk';
import { daemon } from '../lib/daemon-api';
import { WorkspaceOperations } from './WorkspaceOperations';

const agents = [
  { id: 'a', name: 'Nova' },
  { id: 'b', name: 'Scout' },
];
const job = (agentId: string, status: AgentJob['status']): AgentJob => ({
  id: agentId + '-job',
  agentId,
  status,
  title: `${agentId} assignment`,
  prompt: 'Inspect the output',
  goalId: null,
  requestKey: agentId,
  revision: 2,
  attempt: 1,
  maxAttempts: 3,
  requiresApproval: true,
  approvedAtMs: null,
  attempts: [],
  createdAtMs: 1,
  updatedAtMs: 2,
  startedAtMs: null,
  finishedAtMs: null,
  result: null,
  error: null,
});
beforeEach(() => {
  vi.spyOn(daemon, 'goals').mockResolvedValue([]);
  vi.spyOn(daemon, 'agentJobs').mockImplementation(async (id) => [
    job(id, id === 'a' ? 'awaiting_approval' : 'completed'),
  ]);
});
afterEach(() => vi.restoreAllMocks());

it('loads all agents, filters the queue and opens the selected assignment', async () => {
  render(<WorkspaceOperations agents={agents} online onChat={vi.fn()} />);
  const queue = screen.getByRole('region', { name: 'Assignment queue' });
  await within(queue).findByRole('button', { name: /a assignment/ });
  expect(
    within(queue).getByRole('button', { name: /b assignment/ }),
  ).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: /^Approval/ }));
  expect(
    within(queue).queryByRole('button', { name: /b assignment/ }),
  ).not.toBeInTheDocument();
  fireEvent.click(within(queue).getByRole('button', { name: /a assignment/ }));
  expect(
    await screen.findByRole('button', { name: 'Approve and start' }),
  ).toBeVisible();
  expect(screen.queryByLabelText('Run title')).not.toBeInTheDocument();
});

it('reports partial failures without hiding available assignments', async () => {
  vi.mocked(daemon.agentJobs).mockImplementation(async (id) => {
    if (id === 'b') throw new Error('Unavailable');
    return [job(id, 'awaiting_approval')];
  });
  render(<WorkspaceOperations agents={agents} online onChat={vi.fn()} />);
  expect(await screen.findByRole('alert')).toHaveTextContent('Scout');
  expect(screen.getByRole('button', { name: /a assignment/ })).toBeVisible();
});

it('does not fetch or allow dispatch while offline', () => {
  render(
    <WorkspaceOperations agents={agents} online={false} onChat={vi.fn()} />,
  );
  expect(screen.getByRole('button', { name: 'New assignment' })).toBeDisabled();
  expect(daemon.agentJobs).not.toHaveBeenCalled();
  expect(screen.getByText(/Reconnect/)).toBeVisible();
});

it('opens a new assignment for the chosen agent', async () => {
  render(<WorkspaceOperations agents={agents} online onChat={vi.fn()} />);
  fireEvent.click(screen.getByRole('button', { name: 'New assignment' }));
  fireEvent.change(screen.getByLabelText('Assign to'), {
    target: { value: 'b' },
  });
  expect(await screen.findByLabelText('Run title')).toBeVisible();
  expect(screen.getByLabelText('Assign to')).toHaveValue('b');
});

it('refreshes selected detail when a proposal changes elsewhere', async () => {
  const pending = job('a', 'awaiting_approval');
  vi.mocked(daemon.agentJobs).mockResolvedValue([pending]);
  render(<WorkspaceOperations agents={[agents[0]]} online onChat={vi.fn()} />);
  fireEvent.click(await screen.findByRole('button', { name: /a assignment/ }));
  expect(
    await screen.findByRole('button', { name: 'Approve and start' }),
  ).toBeVisible();
  vi.mocked(daemon.agentJobs).mockResolvedValue([
    { ...pending, status: 'cancelled', revision: 3 },
  ]);
  fireEvent.click(screen.getByRole('button', { name: 'Refresh queue' }));
  await within(
    screen.getByRole('region', { name: 'Assignment detail' }),
  ).findByText('Cancelled');
  expect(
    screen.queryByRole('button', { name: 'Approve and start' }),
  ).not.toBeInTheDocument();
});

it('opens retained assignments offline without loading forever or enabling actions', async () => {
  const view = render(
    <WorkspaceOperations agents={agents} online onChat={vi.fn()} />,
  );
  await screen.findByRole('button', { name: /a assignment/ });
  view.rerender(
    <WorkspaceOperations agents={agents} online={false} onChat={vi.fn()} />,
  );
  fireEvent.click(screen.getByRole('button', { name: /a assignment/ }));
  expect(
    screen.getByRole('button', { name: 'Approve and start' }),
  ).toBeDisabled();
  expect(screen.queryByText('Loading runs…')).not.toBeInTheDocument();
});

it('resets a removed agent filter and dispatches to a remaining agent', async () => {
  const view = render(
    <WorkspaceOperations agents={agents} online onChat={vi.fn()} />,
  );
  await screen.findByRole('button', { name: /a assignment/ });
  fireEvent.change(screen.getByLabelText('Filter by agent'), {
    target: { value: 'b' },
  });
  view.rerender(
    <WorkspaceOperations agents={[agents[0]]} online onChat={vi.fn()} />,
  );
  expect(screen.getByLabelText('Filter by agent')).toHaveValue('');
  fireEvent.click(screen.getByRole('button', { name: 'New assignment' }));
  expect(screen.getByLabelText('Assign to')).toHaveValue('a');
});

it('keeps stale rows visible after an agent refresh fails', async () => {
  render(<WorkspaceOperations agents={[agents[0]]} online onChat={vi.fn()} />);
  await screen.findByRole('button', { name: /a assignment/ });
  vi.mocked(daemon.agentJobs).mockRejectedValue(new Error('Connection lost'));
  fireEvent.click(screen.getByRole('button', { name: 'Refresh queue' }));
  expect(await screen.findByRole('alert')).toHaveTextContent(
    'Queue incomplete',
  );
  expect(
    screen.getByRole('button', { name: /a assignment/ }),
  ).toHaveTextContent('Stale');
});

it('provides a visible goal retry while composing', async () => {
  vi.mocked(daemon.goals)
    .mockRejectedValueOnce(new Error('Unavailable'))
    .mockResolvedValue([]);
  render(<WorkspaceOperations agents={[agents[0]]} online onChat={vi.fn()} />);
  fireEvent.click(screen.getByRole('button', { name: 'New assignment' }));
  fireEvent.click(
    await screen.findByRole('button', { name: 'Retry loading goals' }),
  );
  await screen.findByLabelText('Goal');
  expect(daemon.goals).toHaveBeenCalledTimes(2);
});

it('moves accepted output from Review to Completed without losing selection', async () => {
  let current = {
    ...job('a', 'completed'),
    attempts: [
      {
        attempt: 1,
        status: 'completed' as const,
        startedAtMs: 1,
        finishedAtMs: 2,
        result: 'Evidence checked',
        resultTruncated: false,
        error: null,
        review: null,
      },
    ],
  } as AgentJob;
  vi.mocked(daemon.agentJobs).mockImplementation(async () => [current]);
  const accept = vi
    .spyOn(daemon, 'reviewAgentJob')
    .mockImplementation(async () => {
      current = {
        ...current,
        revision: 3,
        attempts: [
          {
            ...current.attempts[0],
            review: { decision: 'accepted', note: '', reviewedAtMs: 3 },
          },
        ],
      };
      return current;
    });
  render(<WorkspaceOperations agents={[agents[0]]} online onChat={vi.fn()} />);
  fireEvent.click(await screen.findByRole('button', { name: /a assignment/ }));
  fireEvent.click(await screen.findByRole('button', { name: 'Accept result' }));
  expect(await screen.findByText('Result accepted')).toBeVisible();
  expect(accept).toHaveBeenCalledWith('a', 'a-job', {
    revision: 2,
    decision: 'accepted',
    note: '',
  });
  expect(screen.getByRole('button', { name: /^Completed/ })).toHaveTextContent(
    '1',
  );
  expect(screen.getByRole('button', { name: /a assignment/ })).toHaveAttribute(
    'aria-pressed',
    'true',
  );
});
