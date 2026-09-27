import { act, renderHook } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Session } from '@animaOS-SWARM/sdk';

import { daemon } from '../lib/daemon-api';
import { runFixture } from '../test/live';
import { sessionFixture } from '../test/sessions';
import {
  useSessionCommands,
  type SessionCommandOptions,
} from './useSessionCommands';

function deferred<Value>() {
  let resolve!: (value: Value) => void;
  const promise = new Promise<Value>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

const CHAT_KEY = 'agent-main\u0000session:room-7';
const readOnly = {
  send: false,
  steer: false,
  stop: false,
  rename: false,
  archive: false,
  delete: false,
  compact: false,
  export: false,
};

function setup(overrides: Partial<SessionCommandOptions> = {}) {
  const session = sessionFixture('room-7', { title: 'Weekend plans' });
  const options: SessionCommandOptions = {
    companionId: 'agent-main',
    canSend: () => true,
    routeSessionId: 'room-7',
    session,
    chatKey: CHAT_KEY,
    draft: '',
    resend: null,
    telegramReady: false,
    activeRun: null,
    updateChat: vi.fn(),
    startChat: vi.fn(),
    queueSend: vi.fn(),
    refreshRuns: vi.fn(),
    setError: vi.fn(),
    navigate: vi.fn(),
    listedSessions: [session],
    upsertSession: vi.fn(),
    setKnownSession: vi.fn(),
    newChat: vi.fn(),
    showCommands: vi.fn(),
    search: vi.fn(),
    chooseModel: vi.fn(),
    rename: vi.fn().mockResolvedValue(true),
    archive: vi.fn().mockResolvedValue(true),
    exportSession: vi.fn().mockResolvedValue(undefined),
    ...overrides,
  };
  const view = renderHook(
    (props: SessionCommandOptions) => useSessionCommands(props),
    { initialProps: options },
  );
  return { ...view, options };
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe('useSessionCommands', () => {
  it('queues the draft, keeping a restored message’s key', () => {
    const { result, options, rerender } = setup({
      draft: ' Book the train ',
      resend: { text: 'Book the train', idempotencyKey: 'key-old' },
    });
    act(() => result.current.send());
    expect(options.updateChat).toHaveBeenCalledWith(CHAT_KEY, {
      draft: '',
      error: null,
      resend: null,
      delivery: null,
    });
    expect(options.queueSend).toHaveBeenCalledWith(
      options.session,
      CHAT_KEY,
      'Book the train',
      'key-old',
      'queue',
    );

    rerender({ ...options, draft: 'Something else' });
    act(() => result.current.send());
    const [, , , key] = vi.mocked(options.queueSend).mock.calls[1];
    expect(key).not.toBe('key-old');
  });

  it('steers only a session whose reply is in progress and can take it', () => {
    const run = runFixture('run_7', { sessionId: 'room-7', status: 'running' });
    const { result, options, rerender } = setup({
      draft: 'also check trains',
      activeRun: run,
    });
    act(() => result.current.steer());
    expect(vi.mocked(options.queueSend).mock.calls[0][4]).toBe('steer');

    rerender({ ...options, activeRun: null });
    act(() => result.current.steer());
    expect(vi.mocked(options.queueSend).mock.calls[1][4]).toBe('queue');
  });

  it('starts a chat from a new conversation and sends nothing it cannot', () => {
    const { result, options, rerender } = setup({
      routeSessionId: null,
      session: null,
      chatKey: 'agent-main\u0000home',
      draft: 'Hello',
    });
    act(() => result.current.send());
    expect(options.startChat).toHaveBeenCalledWith('Hello');

    rerender({ ...options, canSend: () => false });
    act(() => result.current.send());
    expect(options.startChat).toHaveBeenCalledTimes(1);

    const telegram = sessionFixture('telegram:tg-1', { kind: 'telegram' });
    rerender({
      ...options,
      routeSessionId: telegram.id,
      session: telegram,
      telegramReady: false,
    });
    act(() => result.current.send());
    expect(options.queueSend).not.toHaveBeenCalled();
  });

  it('runs the commands the session allows and says why the others cannot run', async () => {
    const compacted: Session = sessionFixture('room-7', {
      title: 'Weekend plans',
      summary: 'Plans so far',
    });
    vi.spyOn(daemon, 'compactSession').mockResolvedValue(compacted);
    const { result, options } = setup();

    await act(async () => result.current.send('/compact'));
    expect(options.upsertSession).toHaveBeenCalledWith(compacted);
    act(() => result.current.send('/rename Offsite'));
    expect(options.rename).toHaveBeenCalledWith(options.session, 'Offsite');
    act(() => result.current.send('/archive'));
    expect(options.archive).toHaveBeenCalledWith(options.session, true);
    act(() => result.current.send('/export'));
    expect(options.exportSession).toHaveBeenCalledWith(options.session);
    act(() => result.current.send('/search trains'));
    expect(options.search).toHaveBeenCalledWith('trains');
    act(() => result.current.send('/help'));
    expect(options.showCommands).toHaveBeenCalled();
    act(() => result.current.send('/model'));
    expect(options.chooseModel).toHaveBeenCalled();
    act(() => result.current.send('/new'));
    expect(options.newChat).toHaveBeenCalled();

    act(() => result.current.send('/stop'));
    expect(options.updateChat).toHaveBeenLastCalledWith(CHAT_KEY, {
      draft: '/stop',
      error: '/stop is not available here.',
    });
    act(() => result.current.send('/rename'));
    expect(options.updateChat).toHaveBeenLastCalledWith(CHAT_KEY, {
      draft: '/rename',
      error: 'Add a title after /rename.',
    });
    expect(options.queueSend).not.toHaveBeenCalled();
  });

  it('shows Compact pending while a manual compaction is in flight (S3b-C)', async () => {
    const pending = deferred<Session>();
    vi.spyOn(daemon, 'compactSession').mockReturnValue(pending.promise);
    const { result, options } = setup();

    expect(result.current.compacting).toBe(false);
    act(() => void result.current.compactSession(options.session!));
    expect(result.current.compacting).toBe(true);

    await act(async () =>
      pending.resolve(sessionFixture('room-7', { summary: 'Plans so far' })),
    );
    expect(result.current.compacting).toBe(false);
  });

  it('clears Compact pending even when the compaction fails', async () => {
    vi.spyOn(daemon, 'compactSession').mockRejectedValue(
      new Error('model unavailable'),
    );
    const { result, options } = setup();

    await act(async () => result.current.compactSession(options.session!));
    expect(result.current.compacting).toBe(false);
    expect(options.setError).toHaveBeenCalledWith('model unavailable');
  });

  it('offers no session command a read-only session lacks', () => {
    const { result, options } = setup({
      session: sessionFixture('job:1', { kind: 'job', capabilities: readOnly }),
      activeRun: runFixture('run_j', { sessionId: 'job:1', status: 'running' }),
    });
    for (const command of ['/stop', '/compact', '/archive', '/export'])
      act(() => result.current.send(command));
    expect(
      vi
        .mocked(options.updateChat)
        .mock.calls.filter(([, patch]) => 'error' in patch && patch.error)
        .map(([, patch]) => patch.error),
    ).toEqual([
      '/stop is not available here.',
      '/compact is not available here.',
      '/archive is not available here.',
      '/export is not available here.',
    ]);
  });

  it('stops a run, reads the ledger again, and shows a failure', async () => {
    const stop = vi
      .spyOn(daemon, 'stopRun')
      .mockResolvedValueOnce(runFixture('run_7', { status: 'cancelled' }))
      .mockRejectedValueOnce(new Error('already finished'));
    const run = runFixture('run_7', { sessionId: 'room-7', status: 'running' });
    const { result, options } = setup({ activeRun: run });

    await act(async () => result.current.send('/stop'));
    expect(stop).toHaveBeenCalledWith('agent-main', 'run_7');
    expect(options.refreshRuns).toHaveBeenCalled();
    await act(async () => result.current.stopRun(run));
    expect(options.setError).toHaveBeenCalledWith('already finished');
  });

  it('sends a run’s message again under a new key, only in its own session', () => {
    const { result, options } = setup();
    act(() =>
      result.current.sendAgain(
        runFixture('run_f', {
          sessionId: 'room-7',
          input: { text: 'Clean the logs', attachmentIds: [], skill: null },
        }),
      ),
    );
    expect(options.queueSend).toHaveBeenCalledWith(
      options.session,
      CHAT_KEY,
      'Clean the logs',
      expect.any(String),
    );
    act(() =>
      result.current.sendAgain(runFixture('run_o', { sessionId: 'chat:b' })),
    );
    expect(options.queueSend).toHaveBeenCalledTimes(1);
  });

  it('never sends again a message the owner did not write', () => {
    const { result, options } = setup();
    let sent = true;
    act(() => {
      sent = result.current.sendAgain(
        runFixture('run_t', {
          sessionId: 'room-7',
          source: 'schedule',
          sourceRef: 'schedule-1',
          status: 'failed',
        }),
      );
    });
    expect(sent).toBe(false);
    expect(options.queueSend).not.toHaveBeenCalled();
  });

  it('routes to another agent’s session by its agent', () => {
    const { result, options } = setup();
    result.current.openTarget({ agentId: 'helper-7', sessionId: 'room-9' });
    result.current.openTarget({ agentId: 'agent-main', sessionId: 'room-7' });
    expect(vi.mocked(options.navigate).mock.calls).toEqual([
      [{ kind: 'session', sessionId: 'room-9', agentId: 'helper-7' }],
      [{ kind: 'session', sessionId: 'room-7' }],
    ]);
  });
});
