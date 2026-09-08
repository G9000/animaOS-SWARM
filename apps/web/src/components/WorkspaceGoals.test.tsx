import {
  render,
  screen,
  fireEvent,
  waitFor,
  act,
} from '@testing-library/react';
import { beforeEach, afterEach, it, expect, vi } from 'vitest';
import type { GoalView, AgentJob } from '@animaOS-SWARM/sdk';
import { daemon } from '../lib/daemon-api';
import { WorkspaceGoals } from './WorkspaceGoals';
const goal: GoalView = {
  id: 'g',
  title: 'Launch',
  objective: 'Ship a report',
  requestKey: 'key',
  status: 'active',
  revision: 3,
  maxAttempts: 4,
  createdAtMs: 1,
  updatedAtMs: 1,
  consumedAttempts: 1,
  reservedAttempts: 1,
  remainingAttempts: 2,
  jobCount: 2,
  acceptedOutputs: 1,
};
beforeEach(() => sessionStorage.clear());
afterEach(() => vi.restoreAllMocks());
it('retains uncertain creation keys and never starts a run when creating a goal', async () => {
  vi.spyOn(daemon, 'goals').mockResolvedValue([]);
  const create = vi
    .spyOn(daemon, 'createGoal')
    .mockRejectedValue(new Error('Offline'));
  const run = vi.spyOn(daemon, 'createAgentJob');
  const view = render(<WorkspaceGoals agents={[]} />);
  fireEvent.change(screen.getByLabelText('Goal title'), {
    target: { value: 'Launch' },
  });
  fireEvent.change(screen.getByLabelText('Objective'), {
    target: { value: 'Ship' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Create goal' }));
  await screen.findByText(/Offline/);
  const input = create.mock.calls[0][0];
  view.unmount();
  render(<WorkspaceGoals agents={[]} />);
  fireEvent.click(screen.getByRole('button', { name: 'Create goal' }));
  await waitFor(() => expect(create).toHaveBeenCalledTimes(2));
  expect(create.mock.calls[1][0]).toEqual(input);
  expect(run).not.toHaveBeenCalled();
});
it('shows aggregate counts and changes status using its revision', async () => {
  vi.spyOn(daemon, 'goals').mockResolvedValue([goal]);
  vi.spyOn(daemon, 'goalJobs').mockResolvedValue([]);
  const status = vi
    .spyOn(daemon, 'setGoalStatus')
    .mockResolvedValue({ ...goal, status: 'paused', revision: 4 });
  render(<WorkspaceGoals agents={[]} />);
  fireEvent.click(await screen.findByRole('button', { name: 'Launch' }));
  expect(screen.getByText('1 consumed')).toBeInTheDocument();
  expect(screen.getByText('1 reserved')).toBeInTheDocument();
  expect(screen.getByText('2 remaining')).toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Pause goal' }));
  await waitFor(() =>
    expect(status).toHaveBeenCalledWith('g', { revision: 3, status: 'paused' }),
  );
});
it('ignores late outputs from a previously selected goal and reports unavailable data', async () => {
  vi.spyOn(daemon, 'goals').mockResolvedValue([
    goal,
    { ...goal, id: 'other', title: 'Other' },
  ]);
  let finish!: (jobs: AgentJob[]) => void;
  vi.spyOn(daemon, 'goalJobs')
    .mockImplementationOnce(() => new Promise((resolve) => (finish = resolve)))
    .mockRejectedValue(new Error('Offline'));
  render(<WorkspaceGoals agents={[]} />);
  fireEvent.click(await screen.findByRole('button', { name: 'Launch' }));
  fireEvent.click(screen.getByRole('button', { name: 'Other' }));
  await screen.findByText(/Could not load goal outputs/);
  await act(async () => finish([]));
  expect(screen.queryByText('No linked runs yet.')).not.toBeInTheDocument();
});

it('groups saved outputs by their agent and completes only after all work is accepted', async () => {
  const job: AgentJob = {
    id: 'j',
    agentId: 'worker',
    goalId: 'g',
    title: 'Research',
    prompt: 'Work',
    requestKey: 'j',
    status: 'completed',
    revision: 2,
    attempt: 1,
    maxAttempts: 3,
    requiresApproval: false,
    approvedAtMs: null,
    createdAtMs: 1,
    updatedAtMs: 2,
    startedAtMs: 1,
    finishedAtMs: 2,
    result: 'Report',
    error: null,
    attempts: [
      {
        attempt: 1,
        status: 'completed',
        startedAtMs: 1,
        finishedAtMs: 2,
        result: '<script>Report</script>',
        error: null,
        resultTruncated: true,
        review: {
          decision: 'accepted',
          note: 'Good evidence',
          reviewedAtMs: 3,
        },
      },
    ],
  };
  vi.spyOn(daemon, 'goals').mockResolvedValue([goal]);
  vi.spyOn(daemon, 'goalJobs').mockResolvedValue([job]);
  const status = vi
    .spyOn(daemon, 'setGoalStatus')
    .mockResolvedValue({ ...goal, status: 'completed', revision: 4 });
  render(<WorkspaceGoals agents={[{ id: 'worker', name: 'Researcher' }]} />);
  fireEvent.click(await screen.findByRole('button', { name: 'Launch' }));
  expect(await screen.findByText(/Researcher/)).toBeInTheDocument();
  expect(screen.getByText('<script>Report</script>')).toBeInTheDocument();
  expect(screen.getByText(/Output was truncated/)).toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Complete goal' }));
  await waitFor(() =>
    expect(status).toHaveBeenCalledWith('g', {
      revision: 3,
      status: 'completed',
    }),
  );
});
