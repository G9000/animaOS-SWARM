import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';

import { emptyLiveRun } from '../../lib/session-events';
import type { ToolStep } from '../../lib/transcript';
import type { ChatMessage } from '../../lib/types';
import { runFixture } from '../../test/live';
import { RunActivity, ToolBlock } from './RunActivity';
import { RunOutcomeCard } from './RunOutcomeCard';

function renderMessage(message: ChatMessage) {
  return <p data-role={message.role}>{message.content.text}</p>;
}

const step: ToolStep = {
  toolCallId: 'call_1',
  name: 'calculate',
  argumentsPreview: 'expression: 2+2',
  status: 'success',
  durationMs: 40,
  result: '4',
  truncated: true,
  runId: 'run_1',
  helper: null,
};

describe('RunActivity', () => {
  it('shows a working run with its tool cards and streamed text', async () => {
    const user = userEvent.setup();
    const run = runFixture('run_1', {
      status: 'running',
      startedAtMs: Date.now(),
      input: { text: 'Add these', attachmentIds: [], skill: null },
    });
    render(
      <RunActivity
        agentName="Nova"
        renderMessage={renderMessage}
        live={{
          ...emptyLiveRun(run),
          steps: [{ stepId: 'run_1:1', text: 'Adding now', textOffset: 0 }],
          tools: [
            {
              stepId: 'run_1:1',
              toolCallId: 'call_1',
              name: 'calculate',
              argumentsPreview: '{"expression":"2+2"}',
              argumentsTruncated: false,
              status: 'success',
              durationMs: 40,
              resultPreview: '4',
              truncated: false,
            },
          ],
        }}
      />,
    );

    expect(
      screen.getByRole('region', { name: 'Nova is replying' }),
    ).toBeVisible();
    expect(screen.getByText('Add these')).toBeVisible();
    expect(screen.getByText('Adding now')).toBeVisible();
    expect(screen.getByText(/^Working · 1 step · /)).toBeVisible();
    const card = screen.getByRole('button', { name: /calculate/ });
    expect(card).toHaveAttribute('aria-expanded', 'false');
    await user.click(card);
    expect(card).toHaveAttribute('aria-expanded', 'true');
    expect(screen.getByText('4')).toBeVisible();
  });

  it('offers to cancel a queued message', async () => {
    const onCancelQueued = vi.fn();
    const run = runFixture('run_q', {
      input: { text: 'Later please', attachmentIds: [], skill: null },
    });
    render(
      <RunActivity
        agentName="Nova"
        renderMessage={renderMessage}
        live={emptyLiveRun(run)}
        actions={{ onCancelQueued }}
      />,
    );

    expect(screen.getByText('Later please')).toBeVisible();
    expect(screen.getByText('Queued')).toBeVisible();
    await userEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(onCancelQueued).toHaveBeenCalledWith(run);
  });

  it('shows the compaction phase and a check-in’s run without its prompt', () => {
    const run = runFixture('run_c', {
      status: 'running',
      source: 'schedule',
      startedAtMs: Date.now(),
      input: { text: 'Check in on goals', attachmentIds: [], skill: null },
    });
    render(
      <RunActivity
        agentName="Nova"
        renderMessage={renderMessage}
        live={{ ...emptyLiveRun(run), phase: 'compacting' }}
      />,
    );

    expect(screen.getByRole('status')).toHaveTextContent(
      'Compacting earlier messages…',
    );
    expect(screen.queryByText('Check in on goals')).not.toBeInTheDocument();
  });
});

describe('ToolBlock', () => {
  it('collapses finished steps to their totals', async () => {
    const user = userEvent.setup();
    render(
      <ToolBlock
        steps={[step, { ...step, toolCallId: 'call_2' }]}
        active={false}
      />,
    );

    const toggle = screen.getByRole('button', { name: 'Used 2 tools · <1s' });
    expect(
      screen.queryByRole('button', { name: /calculate/ }),
    ).not.toBeInTheDocument();
    await user.click(toggle);
    await user.click(screen.getAllByRole('button', { name: /calculate/ })[0]);
    expect(screen.getByText('Result shortened to 2 KiB.')).toBeVisible();
  });

  it('opens a helper’s session from its card', async () => {
    const onOpenSession = vi.fn();
    const helper: ToolStep = {
      ...step,
      name: 'spawn_helper',
      status: 'running',
      result: null,
      helper: { label: 'Researcher', agentId: 'helper-7' },
    };
    render(
      <ToolBlock
        steps={[helper]}
        active
        elapsedMs={2_000}
        actions={{
          helperSession: () => ({ agentId: 'helper-7', sessionId: 'room-9' }),
          onOpenSession,
        }}
      />,
    );

    expect(screen.getByText('Helper · Researcher')).toBeVisible();
    expect(screen.getByText('Working…')).toBeVisible();
    await userEvent.click(screen.getByRole('button', { name: 'Open session' }));
    expect(onOpenSession).toHaveBeenCalledWith({
      agentId: 'helper-7',
      sessionId: 'room-9',
    });
  });

  it('renders duplicate tool-call ids without a duplicate-key warning', () => {
    // Some providers (e.g. the Google adapter's `call_{name}` fallback)
    // reuse the same id when a run calls the same tool twice.
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
    render(
      <ToolBlock steps={[step, { ...step, toolCallId: 'call_1' }]} active />,
    );

    expect(
      errorSpy.mock.calls.some((call) =>
        String(call[0]).toLowerCase().includes('same key'),
      ),
    ).toBe(false);
    errorSpy.mockRestore();
  });
});

describe('RunOutcomeCard', () => {
  it('offers Retry for a failed reply', async () => {
    const onSendAgain = vi.fn();
    const run = runFixture('run_f', {
      status: 'failed',
      error: { code: 'model_error', message: 'provider unavailable' },
    });
    render(<RunOutcomeCard run={run} onSendAgain={onSendAgain} />);

    expect(
      screen.getByText('This reply failed: provider unavailable'),
    ).toBeVisible();
    await userEvent.click(screen.getByRole('button', { name: 'Retry' }));
    expect(onSendAgain).toHaveBeenCalledWith(run);
  });

  it('warns before sending an interrupted message again when tools had started', () => {
    render(
      <RunOutcomeCard
        run={runFixture('run_i', {
          status: 'interrupted',
          toolsStarted: ['bash', 'write_file'],
          error: { code: 'restart_during_run', message: 'restarted' },
        })}
        onSendAgain={vi.fn()}
      />,
    );

    expect(
      screen.getByText('The daemon restarted while this reply was running.'),
    ).toBeVisible();
    expect(
      screen.getByText(
        'Tools had started (bash, write_file). Check their effects before sending again.',
      ),
    ).toBeVisible();
    expect(screen.getByRole('button', { name: 'Send again' })).toBeVisible();
  });

  it('labels a stopped reply', () => {
    render(
      <RunOutcomeCard
        run={runFixture('run_s', { status: 'cancelled', startedAtMs: 1 })}
      />,
    );
    expect(screen.getByText('Stopped')).toBeVisible();
  });
});
