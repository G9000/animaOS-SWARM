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
  id: 'one',
  agentId: 'a',
  title: 'Review',
  prompt: 'Review work',
  requestKey: 'key',
  status: 'needs_review',
  revision: 4,
  attempt: 1,
  createdAtMs: 1,
  updatedAtMs: 2,
  startedAtMs: 1,
  finishedAtMs: null,
  result: null,
  error: 'Interrupted',
};
beforeEach(() => sessionStorage.clear());
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
  fireEvent.click(screen.getByRole('checkbox'));
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
  fireEvent.click(await screen.findByRole('checkbox'));
  fireEvent.click(screen.getByRole('button', { name: 'Retry run' }));
  expect(await screen.findByRole('alert')).toHaveTextContent('Job changed');
  await waitFor(() => expect(list).toHaveBeenCalledTimes(2));
  expect(screen.getByRole('checkbox')).not.toBeChecked();
  fireEvent.click(screen.getByRole('checkbox'));
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
