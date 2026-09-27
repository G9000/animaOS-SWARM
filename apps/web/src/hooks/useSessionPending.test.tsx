import { act, renderHook } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import type { Run, SessionMessage } from '@animaOS-SWARM/sdk';

import { emptyLiveRun, type LiveRun } from '../lib/session-events';
import { runFixture } from '../test/live';
import type { SessionSend } from './useSessionSends';
import {
  useSessionPending,
  type SessionPendingOptions,
} from './useSessionPending';

const SESSION = { agentId: 'agent-main', sessionId: 'room-7' };

function send(overrides: Partial<SessionSend> = {}): SessionSend {
  return {
    key: 'key-1',
    agentId: 'agent-main',
    sessionId: 'room-7',
    conversation: 'agent-main\u0000session:room-7',
    text: 'also check trains',
    mode: 'steer',
    telegram: false,
    createdAtMs: 10,
    failures: 0,
    steeringRunId: null,
    ...overrides,
  };
}

function joined(overrides: Partial<Run> = {}): Run {
  return runFixture('run_7', {
    sessionId: 'room-7',
    status: 'running',
    createdAtMs: 1,
    startedAtMs: 2,
    ...overrides,
  });
}

function setup(initial: Partial<SessionPendingOptions>) {
  const settle = vi.fn();
  const onRecover = vi.fn();
  const refreshMessages = vi.fn().mockResolvedValue(undefined);
  const view = renderHook(
    (props: Partial<SessionPendingOptions>) =>
      useSessionPending({
        sends: [],
        settle,
        session: SESSION,
        messages: [],
        runs: [],
        refreshMessages,
        onRecover,
        ...props,
      }),
    { initialProps: initial },
  );
  return { ...view, settle, onRecover, refreshMessages };
}

describe('useSessionPending', () => {
  it('shows the open session’s sends with their state', () => {
    const { result } = setup({
      sends: [
        send({ key: 'a', mode: 'queue', text: 'First' }),
        send({ key: 'b', mode: 'queue', text: 'Second', failures: 1 }),
        send({ key: 'c', steeringRunId: 'run_7' }),
        send({ key: 'd', sessionId: 'chat:other', mode: 'queue' }),
      ],
      runs: [emptyLiveRun(joined())],
    });
    expect(result.current.map((item) => [item.key, item.status])).toEqual([
      ['a', 'sending'],
      ['b', 'retrying'],
      ['c', 'steering'],
    ]);
  });

  it('hides a steer while its run shows it, and settles it once history has its key', () => {
    const steering = send({ steeringRunId: 'run_7' });
    const live: LiveRun = {
      ...emptyLiveRun(joined()),
      steers: [{ messageId: 'm-steer', text: 'also check trains' }],
    };
    const { result, rerender, settle } = setup({
      sends: [send()],
      runs: [emptyLiveRun(joined())],
    });
    rerender({ sends: [steering], runs: [live] });
    expect(result.current).toEqual([]);
    expect(settle).not.toHaveBeenCalled();

    const committed: SessionMessage = {
      id: 'm-steer',
      role: 'user',
      text: 'also check trains',
      attachments: [],
      metadata: { clientRequestId: 'key-1', steer: true },
      createdAtMs: 3,
    };
    rerender({ sends: [steering], runs: [live], messages: [committed] });
    expect(settle).toHaveBeenCalledWith('key-1');
  });

  it('keeps a steer whose run the stream no longer shows', () => {
    const steering = send({ steeringRunId: 'run_7' });
    const { result, rerender, settle } = setup({
      sends: [send()],
      runs: [emptyLiveRun(joined())],
    });
    // A reconnect's snapshot replaced the stream's runs.
    rerender({ sends: [steering], runs: [] });
    expect(result.current.map((item) => item.status)).toEqual(['steering']);
    expect(settle).not.toHaveBeenCalled();
  });

  it('settles a steer that became a run of its own, but not on an older run with the same text', () => {
    const older = runFixture('run_6', {
      sessionId: 'room-7',
      input: { text: 'also check trains', attachmentIds: [], skill: null },
    });
    const steering = send({ steeringRunId: 'run_7' });
    const { rerender, settle } = setup({
      sends: [send()],
      runs: [emptyLiveRun(older), emptyLiveRun(joined())],
    });
    rerender({
      sends: [steering],
      runs: [emptyLiveRun(older), emptyLiveRun(joined())],
    });
    expect(settle).not.toHaveBeenCalled();

    const own = runFixture('run_8', {
      sessionId: 'room-7',
      status: 'interrupted',
      createdAtMs: 20,
      input: { text: 'also check trains', attachmentIds: [], skill: null },
    });
    rerender({
      sends: [steering],
      runs: [
        emptyLiveRun(older),
        emptyLiveRun(joined({ status: 'cancelled' })),
        emptyLiveRun(own),
      ],
    });
    expect(settle).toHaveBeenCalledWith('key-1');
  });

  it('moves a steer to recovery only after reading history once its run failed', async () => {
    const steering = send({ steeringRunId: 'run_7' });
    const history = { resolve: () => {} };
    const { rerender, settle, onRecover, refreshMessages } = setup({
      sends: [send()],
      runs: [emptyLiveRun(joined())],
    });
    refreshMessages.mockReturnValue(
      new Promise<void>((resolve) => {
        history.resolve = resolve;
      }),
    );
    rerender({
      sends: [steering],
      runs: [emptyLiveRun(joined({ status: 'failed', finishedAtMs: 5 }))],
    });
    expect(refreshMessages).toHaveBeenCalledTimes(1);
    expect(onRecover).not.toHaveBeenCalled();

    await act(async () => history.resolve());
    expect(onRecover).toHaveBeenCalledWith(steering);
    expect(settle).toHaveBeenCalledWith('key-1');
  });

  it('does not recover a steer that history shows the failed run took', async () => {
    const steering = send({ steeringRunId: 'run_7' });
    const failed = [
      emptyLiveRun(joined({ status: 'failed', finishedAtMs: 5 })),
    ];
    const { rerender, settle, onRecover } = setup({
      sends: [send()],
      runs: [emptyLiveRun(joined())],
    });
    rerender({ sends: [steering], runs: failed });
    await act(async () => {
      rerender({
        sends: [steering],
        runs: failed,
        messages: [
          {
            id: 'm-steer',
            role: 'user',
            text: 'also check trains',
            attachments: [],
            metadata: { clientRequestId: 'key-1' },
            createdAtMs: 3,
          },
        ],
      });
    });
    expect(settle).toHaveBeenCalledWith('key-1');
    expect(onRecover).not.toHaveBeenCalled();
  });
});
