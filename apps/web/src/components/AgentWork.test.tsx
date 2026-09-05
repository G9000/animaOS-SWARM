import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { daemon } from '../lib/daemon-api';
import { AgentTasksView } from './AgentWork';

afterEach(() => vi.restoreAllMocks());

it('retains the task draft when a newer list causes a conflict', async () => {
  vi.spyOn(daemon, 'agentTasks').mockResolvedValue({
    tasks: [],
    revision: 'old',
  });
  const update = vi
    .spyOn(daemon, 'updateAgentTasks')
    .mockRejectedValue(
      new Error('Tasks changed. Refresh before saving again.'),
    );
  render(<AgentTasksView agentId="researcher" name="Researcher" />);
  fireEvent.change(await screen.findByLabelText('New agent task'), {
    target: { value: 'Research competitors' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Add task' }));
  fireEvent.click(screen.getByRole('button', { name: 'Save tasks' }));
  expect(await screen.findByRole('alert')).toHaveTextContent('Tasks changed.');
  expect(screen.getByLabelText('Task 1')).toHaveValue('Research competitors');
  expect(update).toHaveBeenCalledWith('researcher', {
    revision: 'old',
    tasks: [
      {
        content: 'Research competitors',
        activeForm: 'Research competitors',
        status: 'pending',
      },
    ],
  });
});

it('allows drafting but prevents saving while the agent is running', async () => {
  vi.spyOn(daemon, 'agentTasks').mockResolvedValue({
    tasks: [],
    revision: 'old',
  });
  render(<AgentTasksView agentId="researcher" name="Researcher" running />);
  fireEvent.change(await screen.findByLabelText('New agent task'), {
    target: { value: 'Research competitors' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Add task' }));
  expect(screen.getByRole('button', { name: 'Save tasks' })).toBeDisabled();
});
