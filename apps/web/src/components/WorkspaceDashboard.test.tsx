import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { daemon, type DaemonSchedule } from '../lib/daemon-api';
import type { AgentDetail } from '../lib/types';
import { WorkspaceDashboard } from './WorkspaceDashboard';

const agents: AgentDetail[] = [{
  id: 'main', name: 'Anima', provider: 'chatgpt', model: 'gpt-5.5', status: 'Running',
  created_at_ms: 0, toolNames: [], token_usage: { prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 },
  messages: [{ id: 'reply', role: 'Assistant', content: { text: 'The draft is ready for review.' }, created_at_ms: 10 }],
}];
const schedule: DaemonSchedule = {
  id: 'schedule', agentId: 'main', prompt: 'Review the campaign', enabled: true,
  trigger: { type: 'interval', intervalMs: 3600000 }, target: { type: 'workspace' },
  nextDueAtMs: Date.now() + 3600000, importIdempotencyKey: null,
  lastOutcome: { status: 'error', occurredAtMs: 0, errorCode: 'provider_error' },
};
const callbacks = () => ({ onChat: vi.fn(), onOpenWork: vi.fn(), onOpenTeam: vi.fn() });
afterEach(() => vi.restoreAllMocks());

it('combines live work and schedules and routes actions to their owner', async () => {
  const tasks = vi.spyOn(daemon, 'agentTasks').mockResolvedValue({ revision: '1', tasks: [
    { content: 'Draft guidelines', status: 'in_progress', activeForm: 'Drafting' },
    { content: 'Research complete', status: 'completed', activeForm: 'Researching' },
  ] });
  vi.spyOn(daemon, 'listSchedules').mockResolvedValue({ schedules: [schedule] });
  const actions = callbacks();
  const { rerender } = render(<WorkspaceDashboard agents={agents} online {...actions} />);
  expect(await screen.findByText('Draft guidelines')).toBeVisible();
  expect(screen.queryByText('Research complete')).not.toBeInTheDocument();
  expect(screen.getByText('Review the campaign')).toBeVisible();
  expect(within(screen.getByRole('region', { name: 'Needs attention' })).getByRole('button')).toHaveTextContent('Schedule needs review');
  fireEvent.click(screen.getByRole('button', { name: 'View tasks' }));
  expect(actions.onOpenWork).toHaveBeenCalledWith('Tasks');
  fireEvent.click(screen.getByRole('button', { name: 'View schedules' }));
  expect(actions.onOpenWork).toHaveBeenCalledWith('Schedules');
  fireEvent.click(screen.getByRole('button', { name: /The draft is ready/ }));
  expect(actions.onChat).toHaveBeenCalledWith('main');
  rerender(<WorkspaceDashboard agents={agents.map((agent) => ({ ...agent, status: 'Completed' }))} online {...actions} />);
  expect(tasks).toHaveBeenCalledOnce();
});

it('preserves available schedules when tasks fail and retries', async () => {
  const tasks = vi.spyOn(daemon, 'agentTasks').mockRejectedValue(new Error('offline'));
  vi.spyOn(daemon, 'listSchedules').mockResolvedValue({ schedules: [schedule] });
  render(<WorkspaceDashboard agents={agents} online {...callbacks()} />);
  expect(await screen.findByRole('alert')).toHaveTextContent('Some task or schedule data could not be loaded');
  expect(screen.getByText('Review the campaign')).toBeVisible();
  expect(screen.getAllByText('Task data unavailable').length).toBeGreaterThan(0);
  tasks.mockResolvedValue({ tasks: [], revision: '2' });
  fireEvent.click(screen.getByRole('button', { name: 'Refresh dashboard' }));
  await waitFor(() => expect(screen.queryByRole('alert')).not.toBeInTheDocument());
  expect(await screen.findByText(/No open tasks/)).toBeVisible();
});

it('does not report a healthy workspace when offline', async () => {
  const tasks = vi.spyOn(daemon, 'agentTasks');
  render(<WorkspaceDashboard agents={agents} online={false} {...callbacks()} />);
  expect(screen.getByText(/The daemon is offline/)).toBeVisible();
  expect(screen.getByRole('button', { name: 'Refresh dashboard' })).toBeDisabled();
  expect(tasks).not.toHaveBeenCalled();
  expect(screen.queryByText('No failed agent runs or schedule errors reported.')).not.toBeInTheDocument();
});
