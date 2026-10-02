import { act, renderHook } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { emptyLiveRun } from '../lib/session-events';
import type { ToolStep } from '../lib/transcript';
import { approvalFixture, runFixture } from '../test/live';
import { sessionFixture } from '../test/sessions';
import {
  helperSessionTarget,
  useTranscriptActions,
  type TranscriptActionOptions,
} from './useTranscriptActions';

function helperStep(agentId: string | null): ToolStep {
  return {
    stepId: 'run_7:1',
    toolCallId: 'call_h',
    name: 'spawn_helper',
    argumentsPreview: 'name: Researcher',
    status: 'running',
    durationMs: null,
    result: null,
    truncated: false,
    runId: 'run_7',
    helper: { label: 'Researcher', agentId },
  };
}

const helperSession = sessionFixture('room-9', {
  agentId: 'helper-7',
  kind: 'helper',
  parentRunId: 'run_7',
});

describe('helperSessionTarget', () => {
  it('opens the live child run’s session first, then the listed one', () => {
    const child = runFixture('run_h', {
      agentId: 'helper-7',
      sessionId: 'room-live',
      parentRunId: 'run_7',
    });
    expect(
      helperSessionTarget(
        helperStep('helper-7'),
        { run_h: emptyLiveRun(child) },
        [helperSession],
      ),
    ).toEqual({ agentId: 'helper-7', sessionId: 'room-live' });
    expect(
      helperSessionTarget(helperStep('helper-7'), {}, [helperSession]),
    ).toEqual({ agentId: 'helper-7', sessionId: 'room-9' });
  });

  it('opens nothing before the helper is known', () => {
    expect(helperSessionTarget(helperStep(null), {}, [helperSession])).toBe(
      null,
    );
  });
});

describe('useTranscriptActions', () => {
  function actionsFor(overrides: Partial<TranscriptActionOptions>) {
    const options: TranscriptActionOptions = {
      session: sessionFixture('room-7'),
      resendable: true,
      liveRuns: {},
      sessions: [],
      stopRun: vi.fn(),
      cancelPending: vi.fn(),
      sendAgain: vi.fn(),
      compact: vi.fn(),
      compacting: false,
      openSession: vi.fn(),
      ...overrides,
    };
    return { options, ...renderHook(() => useTranscriptActions(options)) };
  }

  it('offers what the session allows through its handlers', () => {
    const { result, options } = actionsFor({});
    const run = runFixture('run_q', { sessionId: 'room-7' });
    result.current.onCancelQueued?.(run);
    result.current.onSendAgain?.(run);
    result.current.onCompact?.();
    expect(options.stopRun).toHaveBeenCalledWith(run);
    expect(options.sendAgain).toHaveBeenCalledWith(run);
    expect(options.compact).toHaveBeenCalledWith(options.session);
    expect(result.current.compacting).toBe(false);
  });

  it('reports a manual compaction in flight (S3b-C)', () => {
    const options: TranscriptActionOptions = {
      session: sessionFixture('room-7'),
      resendable: true,
      liveRuns: {},
      sessions: [],
      stopRun: vi.fn(),
      cancelPending: vi.fn(),
      sendAgain: vi.fn(),
      compact: vi.fn(),
      compacting: true,
      openSession: vi.fn(),
    };
    const { result, rerender } = renderHook(
      (props: TranscriptActionOptions) => useTranscriptActions(props),
      { initialProps: options },
    );
    expect(result.current.compacting).toBe(true);

    rerender({ ...options, compacting: false });
    expect(result.current.compacting).toBe(false);
  });

  it('offers nothing a read-only session cannot do', () => {
    const { result } = actionsFor({
      session: sessionFixture('job:1', {
        kind: 'job',
        capabilities: {
          send: false,
          steer: false,
          stop: false,
          rename: false,
          archive: true,
          delete: false,
          compact: false,
          export: true,
        },
      }),
    });
    expect(result.current.onCancelQueued).toBeUndefined();
    expect(result.current.onSendAgain).toBeUndefined();
    expect(result.current.onCompact).toBeUndefined();
  });

  it('keeps its identity while live runs change, and finds a helper in the newest', () => {
    const options: TranscriptActionOptions = {
      session: sessionFixture('room-7'),
      resendable: true,
      liveRuns: {},
      sessions: [],
      stopRun: vi.fn(),
      cancelPending: vi.fn(),
      sendAgain: vi.fn(),
      compact: vi.fn(),
      compacting: false,
      openSession: vi.fn(),
    };
    const { result, rerender } = renderHook(
      (props: TranscriptActionOptions) => useTranscriptActions(props),
      { initialProps: options },
    );
    const first = result.current;
    expect(first.helperSession?.(helperStep('helper-7'))).toBe(null);

    const child = runFixture('run_h', {
      agentId: 'helper-7',
      sessionId: 'room-live',
      parentRunId: 'run_7',
    });
    rerender({ ...options, liveRuns: { run_h: emptyLiveRun(child) } });
    expect(result.current).toBe(first);
    expect(result.current.helperSession?.(helperStep('helper-7'))).toEqual({
      agentId: 'helper-7',
      sessionId: 'room-live',
    });
  });

  it('sends a run again once, and remembers it was sent', () => {
    const sendAgain = vi.fn(() => true);
    const { result } = actionsFor({ sendAgain });
    const run = runFixture('run_f', { sessionId: 'room-7', status: 'failed' });
    expect(result.current.resentRunIds?.has('run_f')).toBe(false);

    act(() => result.current.onSendAgain?.(run));
    act(() => result.current.onSendAgain?.(run));

    expect(sendAgain).toHaveBeenCalledTimes(1);
    expect(result.current.resentRunIds?.has('run_f')).toBe(true);
  });

  it('does not remember a run that could not be sent again', () => {
    const { result } = actionsFor({ sendAgain: vi.fn(() => false) });
    act(() =>
      result.current.onSendAgain?.(
        runFixture('run_f', { sessionId: 'room-7', status: 'failed' }),
      ),
    );
    expect(result.current.resentRunIds?.has('run_f')).toBe(false);
  });

  it('does not send again where the message cannot go', () => {
    const { result } = actionsFor({ resendable: false });
    expect(result.current.onSendAgain).toBeUndefined();
  });

  it('decides approvals through its handler with one identity across renders', async () => {
    const decideApproval = vi.fn().mockResolvedValue(null);
    const { result, rerender } = actionsFor({
      decideApproval,
      companionId: 'agent-main',
    });
    const first = result.current;
    rerender();
    expect(result.current).toBe(first);
    expect(result.current.companionAgentId).toBe('agent-main');

    const approval = approvalFixture('apr_1');
    await expect(
      result.current.onDecideApproval?.(approval, { decision: 'deny' }),
    ).resolves.toBeNull();
    expect(decideApproval).toHaveBeenCalledWith(approval, {
      decision: 'deny',
    });
  });
});
