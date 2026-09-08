import {
  fireEvent,
  render,
  screen,
  waitFor,
  act,
} from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { DaemonHttpError, type AgentJob } from '@animaOS-SWARM/sdk';
import { daemon } from '../lib/daemon-api';
import { AgentRunsView } from './AgentRuns';

const job: AgentJob = {
  goalId: null,
  id: 'one',
  agentId: 'a',
  title: 'Review',
  prompt: 'Review work',
  requestKey: 'key',
  status: 'needs_review',
  revision: 4,
  attempt: 1,
  maxAttempts: 3,
  requiresApproval: false,
  approvedAtMs: null,
  attempts: [],
  createdAtMs: 1,
  updatedAtMs: 2,
  startedAtMs: 1,
  finishedAtMs: null,
  result: null,
  error: 'Interrupted',
};
beforeEach(() => {
  sessionStorage.clear();
  vi.spyOn(daemon, 'goals').mockResolvedValue([]);
});
it('retains approval and attempt choices while migrating legacy drafts', async () => {
  sessionStorage.setItem(
    'anima:run-draft:a',
    JSON.stringify({ title: 'Legacy', prompt: 'Work', requestKey: 'legacy' }),
  );
  vi.spyOn(daemon, 'agentJobs').mockResolvedValue([]);
  const create = vi
    .spyOn(daemon, 'createAgentJob')
    .mockRejectedValue(new Error('Offline'));
  const view = render(<AgentRunsView agentId="a" />);
  expect(screen.getByLabelText('Maximum attempts')).toHaveValue('3');
  fireEvent.click(
    screen.getByLabelText('Require approval before each attempt'),
  );
  fireEvent.change(screen.getByLabelText('Maximum attempts'), {
    target: { value: '2' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Save proposal' }));
  await screen.findByRole('alert');
  const input = create.mock.calls[0][1];
  expect(input).toMatchObject({ maxAttempts: 2, requiresApproval: true });
  expect(input.requestKey).not.toBe('legacy');
  view.unmount();
  render(<AgentRunsView agentId="a" />);
  fireEvent.click(screen.getByRole('button', { name: 'Save proposal' }));
  await screen.findByRole('alert');
  expect(create.mock.calls[1][1]).toEqual(input);
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

it('retains an uncertain submission key and preserves edits made while submitting', async () => {
  vi.spyOn(daemon, 'agentJobs').mockResolvedValue([]);
  let finish!: (value: AgentJob) => void;
  const create = vi
    .spyOn(daemon, 'createAgentJob')
    .mockRejectedValueOnce(new Error('Offline'))
    .mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
  render(<AgentRunsView agentId="a" />);
  fireEvent.change(screen.getByLabelText('Run title'), {
    target: { value: 'Review' },
  });
  fireEvent.change(screen.getByLabelText('Assignment'), {
    target: { value: 'Review work' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Queue run' }));
  expect(await screen.findByRole('alert')).toHaveTextContent('Offline');
  fireEvent.click(screen.getByRole('button', { name: 'Queue run' }));
  await waitFor(() => expect(create).toHaveBeenCalledTimes(2));
  expect(create.mock.calls[1][1].requestKey).toBe(
    create.mock.calls[0][1].requestKey,
  );
  fireEvent.change(screen.getByLabelText('Assignment'), {
    target: { value: 'Next assignment' },
  });
  await act(async () => finish({ ...job, status: 'queued' }));
  expect(screen.getByLabelText('Assignment')).toHaveValue('Next assignment');
});

it('requires explicit uncertainty acknowledgement and limits recovery controls', async () => {
  vi.spyOn(daemon, 'agentJobs').mockResolvedValue([
    job,
    { ...job, id: 'running', title: 'Active', status: 'running' },
    { ...job, id: 'spent', title: 'Exhausted', status: 'failed', attempt: 3 },
  ]);
  const retry = vi
    .spyOn(daemon, 'retryAgentJob')
    .mockResolvedValue({ ...job, status: 'queued' });
  render(<AgentRunsView agentId="a" />);
  expect(await screen.findByText('Needs review')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Retry run' })).toBeDisabled();
  expect(
    screen.queryByRole('button', { name: 'Cancel queued run' }),
  ).not.toBeInTheDocument();
  fireEvent.click(screen.getByLabelText(/I understand retrying/));
  fireEvent.click(screen.getByRole('button', { name: 'Retry run' }));
  await waitFor(() =>
    expect(retry).toHaveBeenCalledWith('a', 'one', {
      revision: 4,
      acknowledgeUncertain: true,
    }),
  );
});

it('refreshes revision conflicts and requires fresh acknowledgement', async () => {
  const list = vi
    .spyOn(daemon, 'agentJobs')
    .mockResolvedValueOnce([job])
    .mockResolvedValue([{ ...job, revision: 6 }]);
  const retry = vi
    .spyOn(daemon, 'retryAgentJob')
    .mockRejectedValue(new DaemonHttpError(409, { error: 'Job changed' }));
  render(<AgentRunsView agentId="a" />);
  fireEvent.click(await screen.findByLabelText(/I understand retrying/));
  fireEvent.click(screen.getByRole('button', { name: 'Retry run' }));
  expect(await screen.findByRole('alert')).toHaveTextContent('Job changed');
  await waitFor(() => expect(list).toHaveBeenCalledTimes(2));
  expect(screen.getByLabelText(/I understand retrying/)).not.toBeChecked();
  fireEvent.click(screen.getByLabelText(/I understand retrying/));
  fireEvent.click(screen.getByRole('button', { name: 'Retry run' }));
  await waitFor(() =>
    expect(retry).toHaveBeenLastCalledWith('a', 'one', {
      revision: 6,
      acknowledgeUncertain: true,
    }),
  );
});

it('bounds polling even if execution remains running', async () => {
  vi.useFakeTimers();
  const list = vi
    .spyOn(daemon, 'agentJobs')
    .mockResolvedValue([{ ...job, status: 'running' }]);
  render(<AgentRunsView agentId="a" />);
  await act(async () => {
    await Promise.resolve();
  });
  for (let index = 0; index < 65; index++)
    await act(async () => {
      await vi.advanceTimersByTimeAsync(5000);
    });
  expect(list).toHaveBeenCalledTimes(60);
  expect(screen.getByText(/Automatic refresh paused/)).toBeInTheDocument();
});

it('retains uncertain request keys across remounts and replaces keys on input changes', async () => {
  vi.spyOn(daemon, 'agentJobs').mockResolvedValue([]);
  const create = vi
    .spyOn(daemon, 'createAgentJob')
    .mockRejectedValue(new Error('Offline'));
  const view = render(<AgentRunsView agentId="a" />);
  fireEvent.change(screen.getByLabelText('Run title'), {
    target: { value: 'Review' },
  });
  fireEvent.change(screen.getByLabelText('Assignment'), {
    target: { value: 'Review work' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Queue run' }));
  await screen.findByRole('alert');
  const key = create.mock.calls[0][1].requestKey;
  view.unmount();
  render(<AgentRunsView agentId="a" />);
  fireEvent.click(screen.getByRole('button', { name: 'Queue run' }));
  await screen.findByRole('alert');
  expect(create.mock.calls[1][1].requestKey).toBe(key);
  fireEvent.change(screen.getByLabelText('Assignment'), {
    target: { value: 'Changed work' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Queue run' }));
  await waitFor(() => expect(create).toHaveBeenCalledTimes(3));
  expect(create.mock.calls[2][1].requestKey).not.toBe(key);
});

it('does not replace a new agent with a late response or present offline as empty', async () => {
  let finish!: (value: AgentJob[]) => void;
  const list = vi
    .spyOn(daemon, 'agentJobs')
    .mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    )
    .mockRejectedValue(new Error('Offline'));
  const view = render(<AgentRunsView agentId="a" />);
  view.rerender(<AgentRunsView agentId="b" />);
  expect(await screen.findByRole('alert')).toHaveTextContent('Offline');
  await act(async () => finish([job]));
  expect(screen.queryByText('Review')).not.toBeInTheDocument();
  expect(screen.queryByText('No runs yet.')).not.toBeInTheDocument();
  expect(list.mock.calls[0][1]?.signal?.aborted).toBe(true);
});

it('approves pending proposals with the current revision', async () => {
  vi.spyOn(daemon, 'agentJobs').mockResolvedValue([
    { ...job, status: 'awaiting_approval', requiresApproval: true },
  ]);
  const approve = vi
    .spyOn(daemon, 'approveAgentJob')
    .mockResolvedValue({ ...job, status: 'queued' });
  render(<AgentRunsView agentId="a" />);
  fireEvent.click(
    await screen.findByRole('button', { name: 'Approve and start' }),
  );
  await waitFor(() =>
    expect(approve).toHaveBeenCalledWith('a', 'one', { revision: 4 }),
  );
});

it('retains saved output and requires feedback before requesting changes', async () => {
  const completed: AgentJob = {
    ...job,
    status: 'completed',
    maxAttempts: 2,
    requiresApproval: true,
    attempts: [
      {
        attempt: 1,
        status: 'completed',
        startedAtMs: 1,
        finishedAtMs: 2,
        result: '<script>saved output</script>',
        error: null,
        resultTruncated: true,
        review: null,
      },
    ],
  };
  const changed: AgentJob = {
    ...completed,
    revision: 5,
    attempts: [
      {
        ...completed.attempts[0],
        review: {
          decision: 'changes_requested',
          note: 'Add evidence',
          reviewedAtMs: 3,
        },
      },
    ],
  };
  vi.spyOn(daemon, 'agentJobs')
    .mockResolvedValueOnce([completed])
    .mockResolvedValue([changed]);
  const review = vi.spyOn(daemon, 'reviewAgentJob').mockResolvedValue(changed);
  const retry = vi
    .spyOn(daemon, 'retryAgentJob')
    .mockResolvedValue({ ...changed, status: 'awaiting_approval' });
  render(<AgentRunsView agentId="a" />);
  expect(
    await screen.findByText('<script>saved output</script>'),
  ).toBeInTheDocument();
  expect(screen.getByText(/Output was truncated/)).toBeInTheDocument();
  expect(
    screen.getByRole('button', { name: 'Request changes' }),
  ).toBeDisabled();
  fireEvent.change(screen.getByLabelText('Review feedback'), {
    target: { value: 'Add evidence' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Request changes' }));
  await waitFor(() =>
    expect(review).toHaveBeenCalledWith('a', 'one', {
      revision: 4,
      decision: 'changes_requested',
      note: 'Add evidence',
    }),
  );
  fireEvent.click(await screen.findByRole('button', { name: 'Retry run' }));
  await waitFor(() =>
    expect(retry).toHaveBeenCalledWith('a', 'one', {
      revision: 5,
      acknowledgeUncertain: false,
    }),
  );
  expect(screen.getByText('<script>saved output</script>')).toBeInTheDocument();
});

it('accepts completed results once and respects the configured attempt limit', async () => {
  const completed: AgentJob = {
    ...job,
    status: 'completed',
    maxAttempts: 1,
    attempts: [
      {
        attempt: 1,
        status: 'completed',
        startedAtMs: 1,
        finishedAtMs: 2,
        result: 'Saved report',
        error: null,
        resultTruncated: false,
        review: null,
      },
    ],
  };
  const accepted: AgentJob = {
    ...completed,
    revision: 5,
    attempts: [
      {
        ...completed.attempts[0],
        review: { decision: 'accepted', note: '', reviewedAtMs: 3 },
      },
    ],
  };
  vi.spyOn(daemon, 'agentJobs')
    .mockResolvedValueOnce([completed])
    .mockResolvedValue([accepted]);
  const review = vi.spyOn(daemon, 'reviewAgentJob').mockResolvedValue(accepted);
  const retry = vi.spyOn(daemon, 'retryAgentJob');
  render(<AgentRunsView agentId="a" />);
  fireEvent.click(await screen.findByRole('button', { name: 'Accept result' }));
  expect(await screen.findByText('Result accepted')).toBeInTheDocument();
  expect(review).toHaveBeenCalledWith('a', 'one', {
    revision: 4,
    decision: 'accepted',
    note: '',
  });
  expect(
    screen.queryByRole('button', { name: 'Accept result' }),
  ).not.toBeInTheDocument();
  expect(
    screen.queryByRole('button', { name: 'Retry run' }),
  ).not.toBeInTheDocument();
  expect(retry).not.toHaveBeenCalled();
});

it('starts a fresh feedback draft for a new attempt and retains uncertain review feedback', async () => {
  const first: AgentJob = {
    ...job,
    status: 'completed',
    attempts: [
      {
        attempt: 1,
        status: 'completed',
        startedAtMs: 1,
        finishedAtMs: 2,
        result: 'First output',
        error: null,
        resultTruncated: false,
        review: null,
      },
    ],
  };
  const next: AgentJob = {
    ...first,
    attempt: 2,
    revision: 7,
    attempts: [
      {
        ...first.attempts[0],
        review: {
          decision: 'changes_requested',
          note: 'Add evidence',
          reviewedAtMs: 3,
        },
      },
      { ...first.attempts[0], attempt: 2, result: 'Second output' },
    ],
  };
  const list = vi.spyOn(daemon, 'agentJobs').mockResolvedValue([first]);
  vi.spyOn(daemon, 'reviewAgentJob').mockRejectedValue(new Error('Offline'));
  render(<AgentRunsView agentId="a" />);
  fireEvent.change(await screen.findByLabelText('Review feedback'), {
    target: { value: 'Add evidence' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Request changes' }));
  await screen.findByRole('alert');
  expect(screen.getByLabelText('Review feedback')).toHaveValue('Add evidence');
  list.mockResolvedValue([next]);
  fireEvent.click(screen.getByRole('button', { name: 'Refresh runs' }));
  await screen.findByText('Second output');
  expect(screen.getByLabelText('Review feedback')).toHaveValue('');
  expect(
    screen.getByRole('button', { name: 'Request changes' }),
  ).toBeDisabled();
});

it('retains goal selection and changes the request key when its linkage changes', async () => {
  vi.mocked(daemon.goals).mockResolvedValue([
    {
      id: 'g',
      title: 'Launch',
      objective: 'Ship',
      requestKey: 'g',
      status: 'paused',
      revision: 1,
      maxAttempts: 4,
      createdAtMs: 1,
      updatedAtMs: 1,
      consumedAttempts: 1,
      reservedAttempts: 0,
      remainingAttempts: 3,
      jobCount: 1,
      acceptedOutputs: 0,
    },
  ]);
  vi.spyOn(daemon, 'agentJobs').mockResolvedValue([]);
  const create = vi
    .spyOn(daemon, 'createAgentJob')
    .mockRejectedValue(new Error('Offline'));
  sessionStorage.setItem(
    'anima:run-draft:a',
    JSON.stringify({ title: 'Work', prompt: 'Do work', requestKey: 'legacy' }),
  );
  const view = render(<AgentRunsView agentId="a" />);
  expect(
    await screen.findByRole('option', { name: /Launch/ }),
  ).toHaveTextContent('paused');
  expect(screen.getByLabelText('Goal')).toHaveValue('');
  fireEvent.change(screen.getByLabelText('Goal'), { target: { value: 'g' } });
  fireEvent.click(
    screen.getByLabelText('Require approval before each attempt'),
  );
  fireEvent.click(screen.getByRole('button', { name: 'Save proposal' }));
  await screen.findByRole('alert');
  expect(create.mock.calls[0][1].goalId).toBe('g');
  expect(create.mock.calls[0][1].requestKey).not.toBe('legacy');
  view.unmount();
  render(<AgentRunsView agentId="a" />);
  await screen.findByRole('option', { name: /Launch/ });
  expect(screen.getByLabelText('Goal')).toHaveValue('g');
  fireEvent.click(screen.getByRole('button', { name: 'Save proposal' }));
  await screen.findByRole('alert');
  expect(create.mock.calls[1][1].requestKey).toBe(
    create.mock.calls[0][1].requestKey,
  );
});
