import { renderHook } from '@testing-library/react';
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

/** The run the daemon made of the steer: it carries the steer's key. */
function ownRun(id: string, overrides: Partial<Run> = {}): Run {
  return sameText(id, { idempotencyKey: 'key-1', ...overrides });
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

/**
 * The hook with the reads it could start counted: `reads.history` and
 * `reads.ledger` are the reads begun so far (history's `readsStarted`, the
 * ledger's `started`), which a test moves as the page would.
 */
function setup(initial: Partial<SessionPendingOptions>) {
  const reads = { history: 1, ledger: 1 };
  const settle = vi.fn();
  const onRecover = vi.fn();
  const refreshMessages = vi.fn(() => {
    reads.history += 1;
  });
  const refreshRuns = vi.fn();
  const ledger = (landed: number | null) => ({
    landed,
    started: () => reads.ledger,
  });
  const view = renderHook(
    (props: Partial<SessionPendingOptions>) =>
      useSessionPending({
        sends: [],
        settle,
        session: SESSION,
        messages: [],
        runs: [],
        appliedRead: 1,
        readsStarted: () => reads.history,
        ledger: ledger(1),
        refreshMessages,
        refreshRuns,
        onRecover,
        ...props,
      }),
    { initialProps: initial },
  );
  return {
    ...view,
    reads,
    ledger,
    settle,
    onRecover,
    refreshMessages,
    refreshRuns,
  };
}

/** A steer seen in flight, then joined to `run_7`. */
function steered() {
  const view = setup({ sends: [send()], runs: [emptyLiveRun(joined())] });
  view.rerender({ sends: [steering], runs: [emptyLiveRun(joined())] });
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

  it('settles a steer that became a run of its own, found by its key', () => {
    const { rerender, settle } = steered();
    const own = ownRun('run_8', { status: 'interrupted', createdAtMs: 20 });
    rerender({
      sends: [steering],
      runs: [emptyLiveRun(joined({ status: 'cancelled' })), emptyLiveRun(own)],
    });
    expect(settle).toHaveBeenCalledWith('key-1');
  });

  it('never takes another message with the same text for the steer’s own run', () => {
    const view = steered();
    // Sent after the steer, with the same words, under a key of its own.
    const other = sameText('run_8', {
      idempotencyKey: 'key-2',
      createdAtMs: 20,
    });
    view.reads.ledger = 2;
    view.rerender({
      sends: [steering],
      runs: [emptyLiveRun(joined()), emptyLiveRun(other)],
      ledger: view.ledger(2),
    });
    expect(view.settle).not.toHaveBeenCalled();
  });

  it('finds the steer’s own run after leaving the session and coming back', () => {
    const view = steered();
    view.rerender({
      sends: [steering],
      session: { agentId: 'agent-main', sessionId: 'chat:b' },
      ledger: view.ledger(null),
    });
    view.reads.ledger = 3;
    const own = ownRun('run_8', { createdAtMs: 20 });
    view.rerender({
      sends: [steering],
      runs: [emptyLiveRun(joined({ status: 'completed' })), emptyLiveRun(own)],
      ledger: view.ledger(3),
    });
    expect(view.settle).toHaveBeenCalledWith('key-1');
  });

  it('hides a message once a run with its key shows it, so its text shows once', () => {
    const queued = send({ key: 'k-q', mode: 'queue', text: 'Book the train' });
    const { result, rerender, settle } = setup({ sends: [queued] });
    expect(result.current.map((item) => item.key)).toEqual(['k-q']);

    // The stream announced its run before the daemon's answer arrived.
    const run = runFixture('run_q', {
      sessionId: 'room-7',
      idempotencyKey: 'k-q',
      input: { text: 'Book the train', attachmentIds: [], skill: null },
    });
    rerender({ sends: [queued], runs: [emptyLiveRun(run)] });
    expect(result.current).toEqual([]);
    // Only hidden: the queue settles it once its answer arrives.
    expect(settle).not.toHaveBeenCalled();
  });

  it('recovers a failed reply’s steer after an applied history read and a landed ledger read, both begun after the failure', () => {
    const view = steered();
    view.rerender({ sends: [steering], runs: failedRuns });
    // One read of each is asked for; nothing else is started.
    expect(view.refreshMessages).toHaveBeenCalledTimes(1);
    expect(view.refreshRuns).toHaveBeenCalledTimes(1);

    // History's read (read 2) applied; the ledger's has not landed.
    view.rerender({ sends: [steering], runs: failedRuns, appliedRead: 2 });
    expect(view.onRecover).not.toHaveBeenCalled();
    expect(view.refreshRuns).toHaveBeenCalledTimes(1);

    view.reads.ledger = 2;
    view.rerender({
      sends: [steering],
      runs: failedRuns,
      appliedRead: 2,
      ledger: view.ledger(2),
    });
    expect(view.onRecover).toHaveBeenCalledWith(steering);
    expect(view.onRecover).toHaveBeenCalledTimes(1);
    expect(view.settle).toHaveBeenCalledWith('key-1');
  });

  it('starts no reads of its own while history keeps failing, and recovers on the next applied read', () => {
    const view = steered();
    view.rerender({ sends: [steering], runs: failedRuns });
    view.reads.ledger = 2;
    const landed = view.ledger(2);
    // History's reads fail, whatever starts them (polls, events).
    for (let poll = 0; poll < 10; poll += 1) {
      view.reads.history += 1;
      view.rerender({ sends: [steering], runs: failedRuns, ledger: landed });
    }
    expect(view.refreshMessages).toHaveBeenCalledTimes(1);
    expect(view.refreshRuns).toHaveBeenCalledTimes(1);
    expect(view.onRecover).not.toHaveBeenCalled();

    view.rerender({
      sends: [steering],
      runs: failedRuns,
      ledger: landed,
      appliedRead: view.reads.history,
    });
    expect(view.onRecover).toHaveBeenCalledTimes(1);
  });

  it('asks the ledger again at most once per applied history read until one lands', () => {
    const view = steered();
    view.rerender({ sends: [steering], runs: failedRuns });
    // History's own read (read 2) applies, but the ledger's read failed.
    view.rerender({ sends: [steering], runs: failedRuns, appliedRead: 2 });
    expect(view.refreshRuns).toHaveBeenCalledTimes(1);

    // The next poll's read (read 3) applies: one more ledger read.
    view.reads.history = 3;
    view.rerender({ sends: [steering], runs: failedRuns, appliedRead: 3 });
    view.rerender({ sends: [steering], runs: failedRuns, appliedRead: 3 });
    expect(view.refreshRuns).toHaveBeenCalledTimes(2);
    expect(view.onRecover).not.toHaveBeenCalled();

    view.reads.ledger = 3;
    view.rerender({
      sends: [steering],
      runs: failedRuns,
      appliedRead: 3,
      ledger: view.ledger(3),
    });
    expect(view.onRecover).toHaveBeenCalledTimes(1);
  });

  it('does not recover a steer the ledger shows as its own run after the history read', () => {
    const view = steered();
    view.rerender({ sends: [steering], runs: failedRuns });
    view.rerender({ sends: [steering], runs: failedRuns, appliedRead: 2 });
    // The daemon announces the steer's `failed_before_start` run after a
    // second save; the ledger read has it.
    const own = ownRun('run_8', {
      status: 'interrupted',
      createdAtMs: 20,
      error: { code: 'failed_before_start', message: 'send it again' },
    });
    view.reads.ledger = 2;
    view.rerender({
      sends: [steering],
      runs: [...failedRuns, emptyLiveRun(own)],
      appliedRead: 2,
      ledger: view.ledger(2),
    });
    expect(view.settle).toHaveBeenCalledWith('key-1');
    expect(view.onRecover).not.toHaveBeenCalled();
  });

  it('starts over when the session changes during a recovery', () => {
    const view = steered();
    view.rerender({ sends: [steering], runs: failedRuns });
    view.rerender({
      sends: [steering],
      session: { agentId: 'agent-main', sessionId: 'chat:b' },
      ledger: view.ledger(null),
      appliedRead: 0,
    });

    // Back in room-7 its history and ledger are read afresh: the reads
    // that landed before are not enough, nor the reset, empty page.
    view.reads.ledger = 3;
    view.rerender({
      sends: [steering],
      runs: failedRuns,
      appliedRead: 2,
      ledger: view.ledger(3),
    });
    expect(view.onRecover).not.toHaveBeenCalled();
    expect(view.refreshMessages).toHaveBeenCalledTimes(2);
    expect(view.refreshRuns).toHaveBeenCalledTimes(2);
  });

  it('does not recover a steer that history shows the failed run took', () => {
    const view = steered();
    view.rerender({ sends: [steering], runs: failedRuns });
    view.reads.ledger = 2;
    view.rerender({
      sends: [steering],
      runs: failedRuns,
      messages: [steerMessage()],
      appliedRead: 2,
      ledger: view.ledger(2),
    });
    expect(view.settle).toHaveBeenCalledWith('key-1');
    expect(view.onRecover).not.toHaveBeenCalled();
  });
});
