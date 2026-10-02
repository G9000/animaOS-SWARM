import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';

import { emptyLiveRun } from '../../lib/session-events';
import type { ToolStep } from '../../lib/transcript';
import type { ChatMessage } from '../../lib/types';
import { approvalFixture, runFixture } from '../../test/live';
import { PendingMessage, RunActivity, ToolBlock } from './RunActivity';
import { RunOutcomeCard } from './RunOutcomeCard';

function renderMessage(message: ChatMessage) {
  return <p data-role={message.role}>{message.content.text}</p>;
}

const step: ToolStep = {
  stepId: 'run_1:1',
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

  it('keeps its status region mounted so the compacting phase is announced when it starts (S3b-E)', () => {
    const run = runFixture('run_p', {
      status: 'running',
      startedAtMs: Date.now(),
    });
    const { rerender } = render(
      <RunActivity
        agentName="Nova"
        renderMessage={renderMessage}
        live={emptyLiveRun(run)}
      />,
    );
    const region = screen.getByRole('status');
    expect(region).toBeEmptyDOMElement();

    rerender(
      <RunActivity
        agentName="Nova"
        renderMessage={renderMessage}
        live={{ ...emptyLiveRun(run), phase: 'compacting' }}
      />,
    );
    expect(screen.getByRole('status')).toBe(region);
    expect(region).toHaveTextContent('Compacting earlier messages…');
  });

  it('shows the run’s approval cards inline and decides through the transcript actions', async () => {
    const user = userEvent.setup();
    const run = runFixture('run_1', {
      status: 'awaiting_approval',
      startedAtMs: Date.now(),
    });
    const approval = approvalFixture('apr_1');
    const onDecideApproval = vi.fn().mockResolvedValue(null);
    render(
      <RunActivity
        agentName="Nova"
        renderMessage={renderMessage}
        live={{ ...emptyLiveRun(run), approvals: [approval] }}
        actions={{ onDecideApproval, companionAgentId: 'agent-main' }}
      />,
    );

    expect(screen.getByText('Waiting for your approval…')).toBeVisible();
    const card = screen.getByRole('region', { name: 'Approval needed: bash' });
    expect(
      within(card).getByRole('button', { name: 'Always allow' }),
    ).toBeVisible();
    await user.click(within(card).getByRole('button', { name: 'Allow once' }));
    expect(onDecideApproval).toHaveBeenCalledWith(approval, {
      decision: 'allow_once',
    });
    expect(
      await within(card).findByText('Allowed once. Continuing…'),
    ).toBeVisible();
  });

  it('offers no rule for an approval that belongs to another agent', () => {
    const run = runFixture('run_h', {
      agentId: 'helper-7',
      status: 'awaiting_approval',
      startedAtMs: Date.now(),
    });
    render(
      <RunActivity
        agentName="Helper"
        renderMessage={renderMessage}
        live={{
          ...emptyLiveRun(run),
          approvals: [approvalFixture('apr_h', { agentId: 'helper-7' })],
        }}
        actions={{ onDecideApproval: vi.fn(), companionAgentId: 'agent-main' }}
      />,
    );

    expect(
      screen.queryByRole('button', { name: 'Always allow' }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByText('Rules for other agents are not managed here yet.'),
    ).toBeVisible();
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

  it('names its result region in aria-controls only while the card is open (S3b-E)', async () => {
    const user = userEvent.setup();
    render(<ToolBlock steps={[step]} active />);

    const card = screen.getByRole('button', { name: /calculate/ });
    expect(card).toHaveAttribute('aria-expanded', 'false');
    expect(card).not.toHaveAttribute('aria-controls');

    await user.click(card);
    const controlsId = card.getAttribute('aria-controls');
    expect(controlsId).toBeTruthy();
    expect(document.getElementById(controlsId!)).toBeInTheDocument();
    expect(screen.getByText('4')).toBeVisible();
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

  it('renders reused tool-call ids without a duplicate-key warning', () => {
    // Some providers number their calls per response, so two steps of a
    // run reuse one id; a recovered result can sit beside its call's card.
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
    render(
      <ToolBlock
        steps={[
          step,
          { ...step, stepId: 'run_1:2', result: '5' },
          { ...step, result: 'recovered' },
        ]}
        active
      />,
    );

    expect(screen.getAllByRole('listitem')).toHaveLength(3);
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

  it.each([
    { source: 'web', sourceRef: null, offered: true },
    { source: 'api', sourceRef: null, offered: true },
    // The daemon's own note after a calendar write, recorded like an API run.
    { source: 'api', sourceRef: 'calendar-write:w-1', offered: false },
    // An owner turn from the console carries its connector's id.
    { source: 'telegram', sourceRef: 'telegram-1-1', offered: true },
    // An inbound turn (`<connector>:<update>`), which the daemon re-runs.
    { source: 'telegram', sourceRef: 'telegram-1-1:42', offered: false },
    { source: 'telegram', sourceRef: null, offered: false },
    { source: 'schedule', sourceRef: 'schedule-1', offered: false },
    { source: 'job', sourceRef: 'job-1:1', offered: false },
    { source: 'delegation', sourceRef: null, offered: false },
    { source: 'peer', sourceRef: null, offered: false },
  ] as const)(
    'offers Retry and Send again only for a message the owner wrote ($source $sourceRef)',
    ({ source, sourceRef, offered }) => {
      const onSendAgain = vi.fn();
      const run = { source, sourceRef };
      render(
        <>
          <RunOutcomeCard
            run={runFixture('run_f', {
              ...run,
              status: 'failed',
              error: { code: 'model_error', message: 'provider unavailable' },
            })}
            onSendAgain={onSendAgain}
          />
          <RunOutcomeCard
            run={runFixture('run_i', {
              ...run,
              status: 'interrupted',
              error: { code: 'restart_before_start', message: 'restarted' },
            })}
            onSendAgain={onSendAgain}
          />
        </>,
      );

      expect(screen.queryAllByRole('button', { name: 'Retry' })).toHaveLength(
        offered ? 1 : 0,
      );
      expect(
        screen.queryAllByRole('button', { name: 'Send again' }),
      ).toHaveLength(offered ? 1 : 0);
      expect(
        screen.getByText('This reply failed: provider unavailable'),
      ).toBeVisible();
    },
  );

  it('disables Retry and Send again once used', () => {
    const failed = runFixture('run_f', {
      status: 'failed',
      error: { code: 'model_error', message: 'provider unavailable' },
    });
    const interrupted = runFixture('run_i', {
      status: 'interrupted',
      error: { code: 'restart_before_start', message: 'restarted' },
    });
    render(
      <>
        <RunOutcomeCard run={failed} onSendAgain={vi.fn()} resent />
        <RunOutcomeCard run={interrupted} onSendAgain={vi.fn()} resent />
      </>,
    );

    expect(screen.getByRole('button', { name: 'Retry' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Send again' })).toBeDisabled();
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

  it.each([
    {
      code: 'restart_during_run',
      message: 'The daemon restarted during this run.',
      shown: 'The daemon restarted while this reply was running.',
    },
    {
      code: 'restart_before_start',
      message:
        'The daemon restarted before this run started; it is safe to send it again.',
      shown: 'The daemon restarted before this message was sent.',
    },
    {
      code: 'stopped_before_start',
      message:
        'The run was stopped before this message reached it; it is safe to send it again.',
      shown:
        'The run was stopped before this message reached it; it is safe to send it again.',
    },
    {
      code: 'queue_full_before_start',
      message:
        'Eight messages were already waiting when the run this message joined ended; it is safe to send it again.',
      shown:
        'Eight messages were already waiting when the run this message joined ended; it is safe to send it again.',
    },
    {
      code: 'failed_before_start',
      message:
        'The run this message joined failed before reading it; send it again.',
      shown:
        'The run this message joined failed before reading it; send it again.',
    },
    {
      code: 'failed_before_start',
      message: '',
      shown: 'This message wasn’t sent.',
    },
  ])(
    'says why an interrupted message did not run ($code)',
    ({ code, message, shown }) => {
      render(
        <RunOutcomeCard
          run={runFixture('run_i', {
            status: 'interrupted',
            error: { code, message },
          })}
          onSendAgain={vi.fn()}
        />,
      );
      expect(screen.getByText(shown)).toBeVisible();
      expect(screen.getByRole('button', { name: 'Send again' })).toBeVisible();
    },
  );

  it('says an interrupted message without a reason was not sent', () => {
    render(
      <RunOutcomeCard
        run={runFixture('run_i', { status: 'interrupted', error: null })}
      />,
    );
    expect(screen.getByText('This message wasn’t sent.')).toBeVisible();
  });
});

describe('PendingMessage', () => {
  it('offers Cancel while a message is unaccepted, but not for a steer (S3b-I)', async () => {
    const user = userEvent.setup();
    const onCancel = vi.fn();
    const bubble = { key: 'k1', text: 'Book the train', createdAtMs: 1 };
    const { rerender } = render(
      <PendingMessage
        pending={{ ...bubble, status: 'retrying' }}
        onCancel={onCancel}
        renderMessage={renderMessage}
      />,
    );
    await user.click(screen.getByRole('button', { name: 'Cancel sending' }));
    expect(onCancel).toHaveBeenCalledWith('k1');

    rerender(
      <PendingMessage
        pending={{ ...bubble, status: 'sending' }}
        onCancel={onCancel}
        renderMessage={renderMessage}
      />,
    );
    expect(
      screen.getByRole('button', { name: 'Cancel sending' }),
    ).toBeVisible();

    rerender(
      <PendingMessage
        pending={{ ...bubble, status: 'steering' }}
        onCancel={onCancel}
        renderMessage={renderMessage}
      />,
    );
    expect(
      screen.queryByRole('button', { name: 'Cancel sending' }),
    ).not.toBeInTheDocument();
  });
});
