import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import {
  daemon,
  type DaemonSchedule,
  type TelegramConnector,
} from '../lib/daemon-api';
import { AgentProactiveView, AgentTasksView } from './AgentWork';

afterEach(() => vi.restoreAllMocks());

function telegramConnector(approved: boolean): TelegramConnector {
  return {
    id: 'tg-1',
    agentId: 'agent-main',
    roomId: 'telegram:tg-1',
    type: 'telegram',
    bot: { id: '1', username: 'nova_bot', displayName: 'Nova' },
    approvedChat: approved
      ? { id: '42', kind: 'private', title: null, username: 'owner' }
      : null,
    pendingPairing: null,
    status: 'ready',
    enabled: true,
    createdAtMs: 1,
    updatedAtMs: 1,
  };
}

function createdSchedule(target: DaemonSchedule['target']): DaemonSchedule {
  return {
    id: 'schedule-1',
    importIdempotencyKey: null,
    agentId: 'agent-main',
    prompt: 'Check my goals',
    trigger: { type: 'interval', intervalMs: 3_600_000 },
    enabled: true,
    target,
    nextDueAtMs: 2,
    lastFiredAtMs: null,
    lastOutcome: null,
    createdAtMs: 1,
    updatedAtMs: 1,
  };
}

it('keeps Telegram delivery off until a chat is approved, then creates a Telegram check-in', async () => {
  const user = userEvent.setup();
  vi.spyOn(daemon, 'listSchedules').mockResolvedValue({ schedules: [] });
  vi.spyOn(daemon, 'listConnectors')
    .mockResolvedValueOnce({ connectors: [telegramConnector(false)] })
    .mockResolvedValue({ connectors: [telegramConnector(true)] });
  const create = vi
    .spyOn(daemon, 'createSchedule')
    .mockResolvedValue({
      schedule: createdSchedule({ type: 'connector', connectorId: 'tg-1' }),
    });
  render(<AgentProactiveView agentId="agent-main" name="Nova" />);

  await screen.findByText('Proactive work is off. No schedules configured for Nova.');
  const target = screen.getByRole('combobox', { name: 'Deliver to' });
  expect(target).toHaveValue('workspace');
  expect(screen.getByRole('option', { name: 'Telegram' })).toBeDisabled();
  expect(target).toHaveAccessibleDescription(
    'Approve a Telegram chat in Connectors to deliver check-ins there.',
  );

  await user.click(screen.getByRole('button', { name: 'Refresh schedules' }));
  await waitFor(() =>
    expect(screen.getByRole('option', { name: 'Telegram' })).toBeEnabled(),
  );
  await user.selectOptions(target, 'telegram');
  await user.type(
    screen.getByRole('textbox', { name: 'Proactive instructions' }),
    'Check my goals',
  );
  await user.click(screen.getByRole('button', { name: 'Enable schedule' }));

  expect(create).toHaveBeenCalledWith('agent-main', {
    prompt: 'Check my goals',
    trigger: { type: 'interval', intervalMs: 3_600_000 },
    target: { type: 'connector', connectorId: 'tg-1' },
    enabled: true,
  });
});

it('creates workspace check-ins by default', async () => {
  const user = userEvent.setup();
  vi.spyOn(daemon, 'listSchedules').mockResolvedValue({ schedules: [] });
  vi.spyOn(daemon, 'listConnectors').mockResolvedValue({
    connectors: [telegramConnector(true)],
  });
  const create = vi
    .spyOn(daemon, 'createSchedule')
    .mockResolvedValue({ schedule: createdSchedule({ type: 'workspace' }) });
  render(<AgentProactiveView agentId="agent-main" name="Nova" />);

  await user.type(
    await screen.findByRole('textbox', { name: 'Proactive instructions' }),
    'Check my goals',
  );
  await user.click(screen.getByRole('button', { name: 'Enable schedule' }));

  expect(create).toHaveBeenCalledWith(
    'agent-main',
    expect.objectContaining({ target: { type: 'workspace' } }),
  );
});

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
