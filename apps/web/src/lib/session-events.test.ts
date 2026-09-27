import { describe, expect, it } from 'vitest';
import type { AgentEvent } from '@animaOS-SWARM/sdk';

import {
  EMPTY_LIVE_STATE,
  MAX_FINISHED_LIVE_RUNS,
  MAX_LIVE_STEP_CHARS,
  MAX_LIVE_TOOL_CARDS,
  applyEvent,
  isActiveRun,
  sessionLiveRuns,
  stepRunId,
  trimCommittedRun,
  type LiveState,
} from './session-events';
import {
  deltaEvent,
  progressEvent,
  resyncEvent,
  runEvent,
  runFixture,
  snapshotEvent,
  snapshotRun,
  steeredEvent,
  toolFinishedEvent,
  toolStartedEvent,
} from '../test/live';

function applyAll(
  events: AgentEvent[],
  state: LiveState = EMPTY_LIVE_STATE,
): LiveState {
  return events.reduce(applyEvent, state);
}

const running = runFixture('run_1', { status: 'running', startedAtMs: 2 });

describe('applyEvent', () => {
  it('starts every stream from its snapshot', () => {
    const before = applyAll([
      snapshotEvent([]),
      runEvent('run.queued', runFixture('run_0'), 2),
    ]);
    const state = applyEvent(
      before,
      snapshotEvent([
        snapshotRun(running, { stepId: 'run_1:1', text: 'Hello' }),
      ]),
    );

    expect(state.seq).toBe(1);
    expect(state.epoch).toBe(before.epoch + 1);
    expect(Object.keys(state.runs)).toEqual(['run_1']);
    expect(state.runs.run_1.steps).toEqual([
      { stepId: 'run_1:1', text: 'Hello', textOffset: 0 },
    ]);
  });

  it('ignores events this stream already applied', () => {
    const once = applyAll([
      snapshotEvent([snapshotRun(running)]),
      deltaEvent(running, 'run_1:1', 0, 'Hi', 2),
    ]);

    expect(applyEvent(once, deltaEvent(running, 'run_1:1', 0, 'Hi', 2))).toBe(
      once,
    );
    expect(
      applyEvent(once, deltaEvent(running, 'run_1:1', 2, ' there', 1)),
    ).toBe(once);
    expect(once.runs.run_1.steps[0].text).toBe('Hi');
  });

  it('joins a step mid-stream without repeating text its snapshot carried', () => {
    // "Hi 👋" is 5 UTF-16 units; the next delta was coalesced before the
    // snapshot was taken and starts inside it (Review Focus 1).
    const state = applyAll([
      snapshotEvent([
        snapshotRun(running, { stepId: 'run_1:1', text: 'Hi 👋' }),
      ]),
      deltaEvent(running, 'run_1:1', 3, '👋 there', 2),
      deltaEvent(running, 'run_1:1', 11, '!', 3),
    ]);

    expect(state.runs.run_1.steps[0].text).toBe('Hi 👋 there!');
  });

  it('keeps the offset of text the snapshot left out', () => {
    const state = applyAll([
      snapshotEvent([
        snapshotRun(running, {
          stepId: 'run_1:1',
          text: 'world',
          textOffset: 6,
        }),
      ]),
      deltaEvent(running, 'run_1:1', 8, 'rld and more', 2),
    ]);

    expect(state.runs.run_1.steps[0]).toEqual({
      stepId: 'run_1:1',
      text: 'world and more',
      textOffset: 6,
    });
  });

  it('ignores a delta past the end of its step until the stream resyncs', () => {
    const state = applyAll([
      snapshotEvent([snapshotRun(running, { stepId: 'run_1:1', text: 'Hel' })]),
      deltaEvent(running, 'run_1:1', 5, 'world', 2),
    ]);

    expect(state.runs.run_1.steps[0].text).toBe('Hel');
  });

  it('keeps earlier steps and their tool cards when a new step starts', () => {
    const state = applyAll([
      snapshotEvent([snapshotRun(running)]),
      deltaEvent(running, 'run_1:1', 0, 'Checking', 2),
      toolStartedEvent(
        running,
        'call_1',
        'calculate',
        3,
        '{"expression":"2+2"}',
      ),
      toolFinishedEvent(running, 'call_1', 'calculate', 4, {
        resultPreview: '4',
        durationMs: 40,
      }),
      deltaEvent(running, 'run_1:2', 0, 'It is 4', 5),
    ]);

    expect(state.runs.run_1.steps.map((step) => step.text)).toEqual([
      'Checking',
      'It is 4',
    ]);
    expect(state.runs.run_1.tools).toEqual([
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
    ]);
  });

  it('keeps a card per step when two steps reuse one tool-call id', () => {
    // Some providers number their calls per response, so `call_0` comes
    // back in every step of a run.
    const state = applyAll([
      snapshotEvent([snapshotRun(running)]),
      toolStartedEvent(running, 'call_0', 'search', 2, '{}', 'run_1:1'),
      toolFinishedEvent(running, 'call_0', 'search', 3, {
        stepId: 'run_1:1',
        resultPreview: 'first',
      }),
      toolStartedEvent(running, 'call_0', 'search', 4, '{}', 'run_1:2'),
      // A late start of step 1's call changes nothing.
      toolStartedEvent(running, 'call_0', 'search', 5, '{}', 'run_1:1'),
    ]);

    expect(
      state.runs.run_1.tools.map((tool) => [
        tool.stepId,
        tool.status,
        tool.resultPreview,
      ]),
    ).toEqual([
      ['run_1:1', 'success', 'first'],
      ['run_1:2', 'running', null],
    ]);

    const finished = applyEvent(
      state,
      toolFinishedEvent(running, 'call_0', 'search', 6, {
        stepId: 'run_1:2',
        status: 'error',
        resultPreview: 'second',
      }),
    );
    expect(
      finished.runs.run_1.tools.map((tool) => [
        tool.stepId,
        tool.status,
        tool.resultPreview,
      ]),
    ).toEqual([
      ['run_1:1', 'success', 'first'],
      ['run_1:2', 'error', 'second'],
    ]);
  });

  it('adds a finished tool whose start it never saw', () => {
    const state = applyAll([
      snapshotEvent([snapshotRun(running)]),
      toolFinishedEvent(running, 'call_9', 'read_file', 2, {
        status: 'error',
        resultPreview: 'missing',
      }),
    ]);

    expect(state.runs.run_1.tools).toEqual([
      expect.objectContaining({
        toolCallId: 'call_9',
        status: 'error',
        resultPreview: 'missing',
      }),
    ]);
  });

  it('never moves a finished run back to an earlier status', () => {
    const done = { ...running, status: 'completed' as const, finishedAtMs: 9 };
    const state = applyAll([
      snapshotEvent([]),
      runEvent('run.started', running, 2),
      runEvent('run.completed', done, 3),
      runEvent('run.started', running, 4),
    ]);

    expect(state.runs.run_1.run.status).toBe('completed');
    expect(isActiveRun(state.runs.run_1.run)).toBe(false);
  });

  it('records each steer once and shows a phase until the run moves on', () => {
    const compacting = applyAll([
      snapshotEvent([snapshotRun(running)]),
      progressEvent(running, 'compacting', 2),
    ]);
    expect(compacting.runs.run_1.phase).toBe('compacting');

    const moved = applyAll(
      [
        steeredEvent(running, 'm1', 'also this', 3),
        steeredEvent(running, 'm1', 'also this', 4),
        deltaEvent(running, 'run_1:1', 0, 'Ok', 5),
      ],
      compacting,
    );
    expect(moved.runs.run_1.steers).toEqual([
      { messageId: 'm1', text: 'also this' },
    ]);
    expect(moved.runs.run_1.phase).toBeNull();
  });

  it('asks views to refetch after a resync', () => {
    const state = applyAll([snapshotEvent([]), resyncEvent(12, 2)]);
    expect(state.epoch).toBe(2);
    expect(state.seq).toBe(2);
  });

  it('keeps the newest 50 finished runs', () => {
    let state = applyEvent(EMPTY_LIVE_STATE, snapshotEvent([]));
    for (let index = 0; index <= MAX_FINISHED_LIVE_RUNS; index += 1) {
      state = applyEvent(
        state,
        runEvent(
          'run.completed',
          runFixture(`run_${index}`, {
            status: 'completed',
            finishedAtMs: index + 1,
          }),
          index + 2,
        ),
      );
    }

    expect(Object.keys(state.runs)).toHaveLength(MAX_FINISHED_LIVE_RUNS);
    expect(state.runs.run_0).toBeUndefined();
    expect(state.runs[`run_${MAX_FINISHED_LIVE_RUNS}`]).toBeDefined();
  });

  it('caps tool cards at 50 per run, dropping the oldest first (S3b-B)', () => {
    let state = applyEvent(
      EMPTY_LIVE_STATE,
      snapshotEvent([snapshotRun(running)]),
    );
    for (let index = 0; index <= MAX_LIVE_TOOL_CARDS; index += 1)
      state = applyEvent(
        state,
        toolStartedEvent(running, `call_${index}`, 'search', index + 2),
      );

    expect(state.runs.run_1.tools).toHaveLength(MAX_LIVE_TOOL_CARDS);
    expect(state.runs.run_1.tools[0].toolCallId).toBe('call_1');
    expect(state.runs.run_1.tools[MAX_LIVE_TOOL_CARDS - 1].toolCallId).toBe(
      `call_${MAX_LIVE_TOOL_CARDS}`,
    );
  });

  it('caps a long step without splitting a character', () => {
    const long = `👋${'a'.repeat(MAX_LIVE_STEP_CHARS - 1)}`;
    const state = applyAll([
      snapshotEvent([snapshotRun(running)]),
      deltaEvent(running, 'run_1:1', 0, long, 2),
      deltaEvent(running, 'run_1:1', long.length, 'b', 3),
    ]);
    const step = state.runs.run_1.steps[0];

    expect(step.text.length).toBeLessThanOrEqual(MAX_LIVE_STEP_CHARS);
    expect(step.text.charCodeAt(0)).toBe('a'.charCodeAt(0));
    expect(step.text.endsWith('ab')).toBe(true);
    expect(step.textOffset + step.text.length).toBe(long.length + 1);
  });
});

describe('trimCommittedRun (S3b-B)', () => {
  it('drops a finished run’s steps and tool cards once its messages are committed', () => {
    const done = runFixture('run_1', {
      status: 'completed',
      finishedAtMs: 9,
    });
    const state = applyAll([
      snapshotEvent([snapshotRun(running)]),
      deltaEvent(running, 'run_1:1', 0, 'Working on it', 2),
      toolStartedEvent(running, 'call_1', 'search', 3),
      runEvent('run.completed', done, 4),
    ]);
    expect(state.runs.run_1.steps).not.toEqual([]);
    expect(state.runs.run_1.tools).not.toEqual([]);

    const trimmed = trimCommittedRun(state, 'run_1');

    expect(trimmed.runs.run_1.steps).toEqual([]);
    expect(trimmed.runs.run_1.tools).toEqual([]);
    // Nothing else about the run changes.
    expect(trimmed.runs.run_1.run).toBe(state.runs.run_1.run);
  });

  it('never trims a run still going', () => {
    const state = applyAll([
      snapshotEvent([snapshotRun(running)]),
      deltaEvent(running, 'run_1:1', 0, 'Working on it', 2),
    ]);

    expect(trimCommittedRun(state, 'run_1')).toBe(state);
  });

  it('is a no-op once a run is already trimmed, or unknown', () => {
    const done = runFixture('run_1', { status: 'completed' });
    const state = applyAll([runEvent('run.completed', done, 1)]);

    expect(trimCommittedRun(state, 'run_1')).toBe(state);
    expect(trimCommittedRun(state, 'run_missing')).toBe(state);
  });
});

describe('selectors', () => {
  it('lists one session’s runs oldest first and names a step’s run', () => {
    const later = runFixture('run_a', { createdAtMs: 5 });
    const earlier = runFixture('run_b', { createdAtMs: 3 });
    const elsewhere = runFixture('run_c', { sessionId: 'chat:2' });
    const state = applyEvent(
      EMPTY_LIVE_STATE,
      snapshotEvent([
        snapshotRun(later),
        snapshotRun(earlier),
        snapshotRun(elsewhere),
      ]),
    );

    expect(
      sessionLiveRuns(state, 'agent-main', 'chat:1').map((live) => live.run.id),
    ).toEqual(['run_b', 'run_a']);
    expect(stepRunId('run_a:3')).toBe('run_a');
  });
});
