import { renderHook } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { emptyLiveRun } from '../lib/session-events';
import type { ToolStep } from '../lib/transcript';
import { runFixture } from '../test/live';
import { sessionFixture } from '../test/sessions';
import {
  helperSessionTarget,
  useTranscriptActions,
  type TranscriptActionOptions,
} from './useTranscriptActions';

function helperStep(agentId: string | null): ToolStep {
  return {
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
      sendAgain: vi.fn(),
      compact: vi.fn(),
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

  it('does not send again where the message cannot go', () => {
    const { result } = actionsFor({ resendable: false });
    expect(result.current.onSendAgain).toBeUndefined();
  });
});
