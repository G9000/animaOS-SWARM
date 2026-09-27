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
/** A ledger read that landed before anything in these tests. */
const LEDGER = { requested: 0, landed: 0, runs: [] };

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

const steering = send({ steeringRunId: 'run_7' });

function joined(overrides: Partial<Run> = {}): Run {
  return runFixture('run_7', {
    sessionId: 'room-7',
    status: 'running',
    createdAtMs: 1,
    startedAtMs: 2,
    ...overrides,
  });
}

const failedRuns = [
  emptyLiveRun(joined({ status: 'failed', finishedAtMs: 5 })),
];

function sameText(id: string, overrides: Partial<Run> = {}): Run {
  return runFixture(id, {
    sessionId: 'room-7',
    input: { text: 'also check trains', attachmentIds: [], skill: null },
    ...overrides,
  });
}

function steerMessage(): SessionMessage {
  return {
    id: 'm-steer',
    role: 'user',
    text: 'also check trains',
    attachments: [],
    metadata: { clientRequestId: 'key-1', steer: true },
    createdAtMs: 3,
  };
}

/** A promise a test settles by hand. */
function held<Value>() {
  let resolve!: (value: Value) => void;
  const promise = new Promise<Value>((settle) => {
    resolve = settle;
  });
  return { promise, resolve };
}

function setup(initial: Partial<SessionPendingOptions>) {
  const settle = vi.fn();
  const onRecover = vi.fn();
  const refreshMessages = vi.fn().mockResolvedValue(true);
  let reads = 0;
  const refreshRuns = vi.fn(() => ++reads);
  const view = renderHook(
    (props: Partial<SessionPendingOptions>) =>
      useSessionPending({
        sends: [],
        settle,
        session: SESSION,
        messages: [],
        runs: [],
        ledger: LEDGER,
        refreshMessages,
        refreshRuns,
        onRecover,
        ...props,
      }),
    { initialProps: initial },
  );
  return { ...view, settle, onRecover, refreshMessages, refreshRuns };
}

/** A steer seen in flight, then joined to `run_7`. */
function steered(initial: Partial<SessionPendingOptions> = {}) {
  const view = setup({
    sends: [send()],
    runs: [emptyLiveRun(joined())],
    ...initial,
  });
  view.rerender({
    ...initial,
    sends: [steering],
    runs: [emptyLiveRun(joined())],
  });
  return view;
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
    const live: LiveRun = {
      ...emptyLiveRun(joined()),
      steers: [{ messageId: 'm-steer', text: 'also check trains' }],
    };
    const { result, rerender, settle } = steered();
    rerender({ sends: [steering], runs: [live] });
    expect(result.current).toEqual([]);
    expect(settle).not.toHaveBeenCalled();

    rerender({ sends: [steering], runs: [live], messages: [steerMessage()] });
    expect(settle).toHaveBeenCalledWith('key-1');
  });

  it('keeps a steer whose run the stream no longer shows', () => {
    const { result, rerender, settle } = steered();
    // A reconnect's snapshot replaced the stream's runs.
    rerender({ sends: [steering], runs: [] });
    expect(result.current.map((item) => item.status)).toEqual(['steering']);
    expect(settle).not.toHaveBeenCalled();
  });

  it('settles a steer that became a run of its own, but not on an older run with the same text', () => {
    const older = sameText('run_6');
    const { rerender, settle } = setup({
      sends: [send()],
      runs: [emptyLiveRun(older), emptyLiveRun(joined())],
    });
    rerender({
      sends: [steering],
      runs: [emptyLiveRun(older), emptyLiveRun(joined())],
    });
    expect(settle).not.toHaveBeenCalled();

    const own = sameText('run_8', { status: 'interrupted', createdAtMs: 20 });
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

  it('counts a same-text run from a ledger read begun before the steer as older', () => {
    // The session's first ledger read is still on its way when the steer goes.
    const pending = { requested: 0, landed: null, runs: [] };
    const older = sameText('run_6');
    const { rerender, settle } = setup({
      sends: [send()],
      runs: [emptyLiveRun(joined())],
      ledger: pending,
    });
    const early = { requested: 0, landed: 0, runs: [older] };
    rerender({
      sends: [steering],
      runs: [emptyLiveRun(older), emptyLiveRun(joined())],
      ledger: early,
    });
    expect(settle).not.toHaveBeenCalled();

    // A read begun after the steer may hold the run it became.
    const own = sameText('run_8', { createdAtMs: 20 });
    rerender({
      sends: [steering],
      runs: [emptyLiveRun(older), emptyLiveRun(joined()), emptyLiveRun(own)],
      ledger: { requested: 1, landed: 1, runs: [older, own] },
    });
    expect(settle).toHaveBeenCalledWith('key-1');
  });

  it('moves a steer to recovery only once history and the ledger were read after its run failed', async () => {
    const history = held<boolean>();
    const { rerender, settle, onRecover, refreshMessages, refreshRuns } =
      steered();
    refreshMessages.mockReturnValueOnce(history.promise);
    rerender({ sends: [steering], runs: failedRuns });
    expect(refreshMessages).toHaveBeenCalledTimes(1);
    expect(refreshRuns).toHaveBeenCalledTimes(1);

    await act(async () => history.resolve(true));
    // The ledger read begun after the failure has not landed yet.
    expect(onRecover).not.toHaveBeenCalled();
    rerender({
      sends: [steering],
      runs: failedRuns,
      ledger: { requested: 1, landed: 1, runs: [] },
    });
    expect(onRecover).toHaveBeenCalledWith(steering);
    expect(onRecover).toHaveBeenCalledTimes(1);
    expect(settle).toHaveBeenCalledWith('key-1');
  });

  it('does not recover a steer the ledger shows as its own run after the history read', async () => {
    const { rerender, onRecover, settle } = steered();
    await act(async () => {
      rerender({ sends: [steering], runs: failedRuns });
    });
    // The daemon announces the steer's `failed_before_start` run after a
    // second save; the ledger read has it.
    const own = sameText('run_8', {
      status: 'interrupted',
      createdAtMs: 20,
      error: { code: 'failed_before_start', message: 'send it again' },
    });
    rerender({
      sends: [steering],
      runs: [...failedRuns, emptyLiveRun(own)],
      ledger: { requested: 1, landed: 1, runs: [own] },
    });
    expect(settle).toHaveBeenCalledWith('key-1');
    expect(onRecover).not.toHaveBeenCalled();
  });

  it('never recovers on a history read that was discarded or failed', async () => {
    const { rerender, onRecover, refreshMessages } = steered();
    refreshMessages.mockResolvedValueOnce(false);
    await act(async () => {
      rerender({ sends: [steering], runs: failedRuns });
    });
    await act(async () => {
      rerender({
        sends: [steering],
        runs: failedRuns,
        ledger: { requested: 1, landed: 1, runs: [] },
      });
    });
    // The next pass asks history again (and the ledger with it): nothing
    // moves on the discarded read.
    expect(refreshMessages).toHaveBeenCalledTimes(2);
    expect(onRecover).not.toHaveBeenCalled();
  });

  it('starts over when the session changes during a recovery', async () => {
    const history = held<boolean>();
    const { rerender, onRecover, refreshMessages } = steered();
    refreshMessages.mockReturnValueOnce(history.promise);
    rerender({ sends: [steering], runs: failedRuns });
    rerender({
      sends: [steering],
      session: { agentId: 'agent-main', sessionId: 'chat:b' },
      runs: [],
      ledger: { requested: 1, landed: 1, runs: [] },
    });
    await act(async () => history.resolve(true));

    // Back in room-7 its history is read afresh: nothing recovers against
    // the reset, empty page.
    refreshMessages.mockReturnValueOnce(new Promise(() => undefined));
    rerender({
      sends: [steering],
      runs: failedRuns,
      ledger: { requested: 2, landed: 2, runs: [] },
    });
    expect(onRecover).not.toHaveBeenCalled();
    expect(refreshMessages).toHaveBeenCalledTimes(2);
  });

  it('does not recover a steer that history shows the failed run took', async () => {
    const { rerender, settle, onRecover } = steered();
    rerender({ sends: [steering], runs: failedRuns });
    await act(async () => {
      rerender({
        sends: [steering],
        runs: failedRuns,
        messages: [steerMessage()],
        ledger: { requested: 1, landed: 1, runs: [] },
      });
    });
    expect(settle).toHaveBeenCalledWith('key-1');
    expect(onRecover).not.toHaveBeenCalled();
  });
});
