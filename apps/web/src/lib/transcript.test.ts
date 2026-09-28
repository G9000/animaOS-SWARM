import { describe, expect, it } from 'vitest';

import { emptyLiveRun } from './session-events';
import {
  argumentsSummary,
  buildHistory,
  buildTranscript,
  delegatedTaskText,
  formatElapsed,
  liveToolSteps,
  mergeSessionRuns,
  placeRuns,
  previewSummary,
  type TranscriptItem,
} from './transcript';
import type { ChatMessage } from './types';
import { runFixture } from '../test/live';

function message(
  id: string,
  role: ChatMessage['role'],
  text: string,
  metadata: Record<string, unknown> = {},
  createdAtMs = 1,
): ChatMessage {
  return {
    id,
    role,
    content: { text, metadata },
    created_at_ms: createdAtMs,
  };
}

function kinds(items: TranscriptItem[]): string[] {
  return items.map((item) => item.kind);
}

describe('buildTranscript', () => {
  it('turns a run’s tool calls and results into one block of steps', () => {
    const items = buildTranscript({
      messages: [
        message('u1', 'User', 'What is 2+2?', { runId: 'run_1' }),
        message('a1', 'Assistant', 'Let me check.', {
          runId: 'run_1',
          stepId: 'run_1:1',
          toolCalls: [
            { id: 'call_1', name: 'calculate', args: { expression: '2+2' } },
          ],
        }),
        message('t1', 'Tool', '4', {
          runId: 'run_1',
          toolCallId: 'call_1',
          toolStatus: 'success',
          toolDurationMs: 40,
        }),
        message('a2', 'Assistant', 'It is 4.', {
          runId: 'run_1',
          stepId: 'run_1:2',
        }),
      ],
    });

    expect(kinds(items)).toEqual(['message', 'message', 'tools', 'message']);
    const block = items[2];
    expect(block.kind === 'tools' && block.messageIds).toEqual(['a1', 't1']);
    expect(block.kind === 'tools' && block.steps).toEqual([
      {
        stepId: 'run_1:1',
        toolCallId: 'call_1',
        name: 'calculate',
        argumentsPreview: 'expression: 2+2',
        status: 'success',
        durationMs: 40,
        result: '4',
        truncated: false,
        runId: 'run_1',
        helper: null,
      },
    ]);
  });

  // Ruling 3: the daemon never writes `taskResult` on committed messages
  // (it stays filtered — hosts/rust-daemon EXPOSED_MESSAGE_METADATA). A
  // failed tool result's error lives in the tool message's own JSON text,
  // the shape `content_from_tool_result` (anima-core runtime.rs) writes on
  // the wire: `{"status":"error","data":null,"error":"…"}`.
  it('shows a failed tool’s error and keeps results whose call is not loaded', () => {
    const items = buildTranscript({
      messages: [
        message(
          't0',
          'Tool',
          '{"status":"error","data":null,"error":"file not found"}',
          { toolCallId: 'call_0', toolStatus: 'error', toolDurationMs: 12 },
        ),
        message('a1', 'Assistant', '', {
          toolCalls: [{ id: 'call_1', name: 'bash', args: {} }],
        }),
      ],
    });

    expect(kinds(items)).toEqual(['tools']);
    const block = items[0];
    expect(block.kind === 'tools' && block.steps).toEqual([
      expect.objectContaining({
        toolCallId: 'call_0',
        // The call itself is on an older, unloaded page: nothing named
        // it, so the card falls back to the generic label.
        name: 'tool',
        status: 'error',
        result: 'file not found',
        durationMs: 12,
      }),
      // A stored call with no recorded result is not left spinning.
      expect.objectContaining({
        toolCallId: 'call_1',
        status: 'error',
        result: null,
      }),
    ]);
  });

  // A result can arrive detached from its call's block: a recovered or
  // delayed result recorded after the run moved on (spec §4.6's restart
  // recovery), here after an unrelated later run's own turn closed the
  // block. Within the same run it answers its call's earlier card, rather
  // than leaving that card stuck at ✗ beside a second card (T17).
  it('merges a recovered result into its call’s earlier card in the same run', () => {
    const messages = [
      message('a0', 'Assistant', 'On it.', {
        runId: 'run_1',
        toolCalls: [{ id: 'call_9', name: 'search', args: {} }],
      }),
      message('a1', 'Assistant', 'Retrying separately.', {
        runId: 'run_2',
      }),
      message(
        't0',
        'Tool',
        '{"status":"error","data":null,"error":"timed out"}',
        { runId: 'run_1', toolCallId: 'call_9', toolStatus: 'error' },
      ),
    ];
    const items = buildTranscript({ messages });

    expect(kinds(items)).toEqual(['message', 'tools', 'message']);
    const block = items[1];
    expect(block.kind === 'tools' && block.steps).toEqual([
      expect.objectContaining({
        toolCallId: 'call_9',
        name: 'search',
        status: 'error',
        result: 'timed out',
      }),
    ]);
    expect(block.kind === 'tools' && block.messageIds).toEqual(['a0', 't0']);
    // The run's last item is still its card, not the later run's turn.
    expect(buildHistory({ messages }).lastOfRun.get('run_1')).toBe(1);
  });

  // Ruling 3 (web part): an orphan card — a tool result whose call is not
  // in the same block, and not known to be of the same run — takes its
  // name from the matching assistant message's `toolCalls` (found anywhere
  // in the loaded history), never from a `toolName` metadata key the
  // daemon does not write.
  it('names an orphan card from the assistant message that made its call', () => {
    const items = buildTranscript({
      messages: [
        message('a0', 'Assistant', 'On it.', {
          toolCalls: [{ id: 'call_9', name: 'search', args: {} }],
        }),
        message('a1', 'Assistant', 'Retrying separately.'),
        message('t0', 'Tool', '{"status":"success","data":"found"}', {
          toolCallId: 'call_9',
          toolStatus: 'success',
        }),
      ],
    });

    expect(kinds(items)).toEqual(['message', 'tools', 'message', 'tools']);
    const orphanBlock = items[3];
    expect(orphanBlock.kind === 'tools' && orphanBlock.steps).toEqual([
      expect.objectContaining({
        toolCallId: 'call_9',
        name: 'search',
        status: 'success',
      }),
    ]);
  });

  it('reads a legacy tool result’s status from its JSON text when `toolStatus` is absent', () => {
    // Pre-M3 tool messages carry no `toolStatus` marker at all.
    const items = buildTranscript({
      messages: [
        message('a1', 'Assistant', '', {
          toolCalls: [{ id: 'call_1', name: 'bash', args: {} }],
        }),
        message('t1', 'Tool', '{"status":"error","data":null,"error":"boom"}', {
          toolCallId: 'call_1',
        }),
      ],
    });

    expect(kinds(items)).toEqual(['tools']);
    const block = items[0];
    expect(block.kind === 'tools' && block.steps).toEqual([
      expect.objectContaining({
        toolCallId: 'call_1',
        status: 'error',
        result: 'boom',
      }),
    ]);
  });

  it('starts a new tool block when the run changes, even with no message in between', () => {
    const items = buildTranscript({
      messages: [
        message('a1', 'Assistant', '', {
          runId: 'run_a',
          toolCalls: [{ id: 'c1', name: 'search', args: {} }],
        }),
        message('t1', 'Tool', 'ok', {
          runId: 'run_a',
          toolCallId: 'c1',
          toolStatus: 'success',
        }),
        // run_b's first visible message is itself a tool call, with
        // nothing textual closing run_a's block first.
        message('a2', 'Assistant', '', {
          runId: 'run_b',
          toolCalls: [{ id: 'c2', name: 'write', args: {} }],
        }),
      ],
    });

    expect(kinds(items)).toEqual(['tools', 'tools']);
    expect(
      items[0].kind === 'tools' && items[0].steps.map((s) => s.toolCallId),
    ).toEqual(['c1']);
    expect(
      items[1].kind === 'tools' && items[1].steps.map((s) => s.toolCallId),
    ).toEqual(['c2']);
  });

  it('collapses a multi-step run’s tool calls into one block', () => {
    const items = buildTranscript({
      messages: [
        message('a1', 'Assistant', '', {
          runId: 'run_1',
          stepId: 'run_1:1',
          toolCalls: [{ id: 'call_1', name: 'search', args: {} }],
        }),
        message('t1', 'Tool', 'found', {
          runId: 'run_1',
          toolCallId: 'call_1',
          toolStatus: 'success',
        }),
        message('a2', 'Assistant', '', {
          runId: 'run_1',
          stepId: 'run_1:2',
          toolCalls: [{ id: 'call_2', name: 'write', args: {} }],
        }),
        message('t2', 'Tool', 'done', {
          runId: 'run_1',
          toolCallId: 'call_2',
          toolStatus: 'success',
        }),
      ],
    });

    expect(kinds(items)).toEqual(['tools']);
    expect(
      items[0].kind === 'tools' && items[0].steps.map((s) => s.toolCallId),
    ).toEqual(['call_1', 'call_2']);
  });

  it('gives a result to the call of its own step when steps reuse a call id', () => {
    // Step 1's call never got a result; step 2 reuses its id.
    const items = buildTranscript({
      messages: [
        message('a1', 'Assistant', '', {
          runId: 'run_1',
          stepId: 'run_1:1',
          toolCalls: [{ id: 'call_0', name: 'search', args: {} }],
        }),
        message('a2', 'Assistant', '', {
          runId: 'run_1',
          stepId: 'run_1:2',
          toolCalls: [{ id: 'call_0', name: 'search', args: {} }],
        }),
        message('t2', 'Tool', 'found', {
          runId: 'run_1',
          stepId: 'run_1:2',
          toolCallId: 'call_0',
          toolStatus: 'success',
        }),
      ],
    });

    expect(
      items[0].kind === 'tools' &&
        items[0].steps.map((step) => [step.stepId, step.status, step.result]),
    ).toEqual([
      ['run_1:1', 'error', null],
      ['run_1:2', 'success', 'found'],
    ]);
  });

  it('hides an old failed run this page never saw stream, because it is old — not because it failed', () => {
    const oldFailed = runFixture('run_old_f', {
      status: 'failed',
      createdAtMs: 1,
      error: { code: 'model_error', message: 'gone' },
    });
    const items = buildTranscript({
      messages: [message('u1', 'User', 'hi', {}, 100)],
      runs: [emptyLiveRun(oldFailed)],
    });

    expect(items.map((item) => item.key)).toEqual(['u1']);
  });

  it('keeps same-anchor outcome inserts in the order they were given', () => {
    // buildTranscript's own contract does not require a pre-deduplicated
    // `runs` list (mergeSessionRuns is what normally deduplicates before
    // this is called); if a caller passes the same run twice — e.g. a
    // stale copy from a race between the live stream and a ledger refetch
    // — both outcome inserts share the same anchor and must not flip
    // relative order.
    const first = runFixture('run_dup', {
      status: 'failed',
      startedAtMs: 1,
      error: { code: 'model_error', message: 'first' },
    });
    const staleCopy = {
      ...first,
      error: { code: 'model_error', message: 'stale' },
    };
    const items = buildTranscript({
      messages: [message('u1', 'User', 'go', { runId: 'run_dup' }, 1)],
      runs: [emptyLiveRun(first), emptyLiveRun(staleCopy)],
    });

    expect(items.map((item) => item.key)).toEqual([
      'u1',
      'outcome:run_dup',
      'outcome:run_dup',
    ]);
    expect(items[1].kind === 'outcome' && items[1].run.error?.message).toBe(
      'first',
    );
    expect(items[2].kind === 'outcome' && items[2].run.error?.message).toBe(
      'stale',
    );
  });

  it('collapses revised drafts and credits delegated turns to their author', () => {
    const items = buildTranscript({
      delegatedBy: 'Nova',
      messages: [
        message(
          'u1',
          'User',
          'Task delegated by workspace manager Nova (agent-main). Return the result and any blockers. Do not delegate further.\n\nCompare vendors',
        ),
        message('a1', 'Assistant', 'Draft one', { revised: true }),
        message('a2', 'Assistant', 'Final comparison'),
      ],
    });

    expect(kinds(items)).toEqual(['delegated', 'revised', 'message']);
    const delegated = items[0];
    expect(delegated.kind === 'delegated' && delegated.from).toBe('Nova');
    expect(
      delegated.kind === 'delegated' && delegated.message.content.text,
    ).toBe('Compare vendors');
  });

  it('places the trimmed divider after the dropped-through message, or first', () => {
    const messages = [
      message('m1', 'User', 'old'),
      message('m2', 'Assistant', 'old reply'),
      message('m3', 'User', 'new'),
    ];

    expect(kinds(buildTranscript({ messages, trimmedThrough: 'm2' }))).toEqual([
      'message',
      'message',
      'trimmed',
      'message',
    ]);
    expect(
      kinds(buildTranscript({ messages, trimmedThrough: 'older' })),
    ).toEqual(['trimmed', 'message', 'message', 'message']);
  });

  it('shows uncommitted runs at the end and outcomes after committed ones', () => {
    const failed = runFixture('run_f', {
      status: 'failed',
      startedAtMs: 2,
      finishedAtMs: 3,
      error: { code: 'model_error', message: 'provider unavailable' },
    });
    const done = runFixture('run_d', { status: 'completed', startedAtMs: 4 });
    const cancelledEarly = runFixture('run_c', {
      status: 'cancelled',
      createdAtMs: 5,
    });
    const interrupted = runFixture('run_i', {
      status: 'interrupted',
      createdAtMs: 6,
      error: { code: 'restart_before_start', message: 'restarted' },
    });
    const oldDone = runFixture('run_o', {
      status: 'completed',
      createdAtMs: 0,
    });
    const active = runFixture('run_a', { status: 'running', createdAtMs: 7 });
    const queued = runFixture('run_q', { createdAtMs: 8 });

    const items = buildTranscript({
      messages: [
        message('u1', 'User', 'first', { runId: 'run_f' }, 1),
        message('u2', 'User', 'second', { runId: 'run_d' }, 4),
        message('a2', 'Assistant', 'answer', { runId: 'run_d' }, 4),
      ],
      runs: [
        emptyLiveRun(oldDone),
        emptyLiveRun(failed),
        emptyLiveRun(done),
        emptyLiveRun(cancelledEarly),
        emptyLiveRun(interrupted),
        emptyLiveRun(active),
        emptyLiveRun(queued),
      ],
      pending: [{ key: 'k1', text: 'next', createdAtMs: 9, status: 'sending' }],
    });

    expect(items.map((item) => item.key)).toEqual([
      'u1',
      'outcome:run_f',
      'u2',
      'a2',
      'run:run_i',
      'outcome:run_i',
      'run:run_a',
      'run:run_q',
      'pending:k1',
    ]);
  });

  it('places a finished run with nothing committed where it happened', () => {
    const failed = runFixture('run_f', {
      status: 'failed',
      createdAtMs: 5,
      startedAtMs: 5,
      finishedAtMs: 6,
      error: { code: 'model_error', message: 'provider unavailable' },
    });
    const interrupted = runFixture('run_i', {
      status: 'interrupted',
      createdAtMs: 12,
      error: { code: 'restart_before_start', message: 'restarted' },
    });
    const active = runFixture('run_a', { status: 'running', createdAtMs: 3 });
    const items = buildTranscript({
      messages: [
        message('u1', 'User', 'first', { runId: 'run_1' }, 1),
        message('a1', 'Assistant', 'reply', { runId: 'run_1' }, 2),
        message('u2', 'User', 'second', { runId: 'run_2' }, 10),
        message('a2', 'Assistant', 'reply', { runId: 'run_2' }, 11),
      ],
      runs: [
        emptyLiveRun(active),
        emptyLiveRun(failed),
        emptyLiveRun(interrupted),
      ],
    });

    expect(items.map((item) => item.key)).toEqual([
      'u1',
      'a1',
      'run:run_f',
      'outcome:run_f',
      'u2',
      'a2',
      'run:run_i',
      'outcome:run_i',
      // A run still going stays at the end, whenever it was accepted.
      'run:run_a',
    ]);
  });

  it('places a run older than the loaded page only once no older page is left', () => {
    const failed = runFixture('run_f', {
      status: 'failed',
      createdAtMs: 1,
      error: { code: 'model_error', message: 'gone' },
    });
    const history = buildHistory({
      messages: [message('u1', 'User', 'hi', {}, 100)],
    });

    expect(
      placeRuns(history, { runs: [emptyLiveRun(failed)] }).map(
        (item) => item.key,
      ),
    ).toEqual(['u1']);
    expect(
      placeRuns(history, {
        runs: [emptyLiveRun(failed)],
        olderHistory: false,
      }).map((item) => item.key),
    ).toEqual(['run:run_f', 'outcome:run_f', 'u1']);
  });

  it('keeps the history’s own items when placing runs', () => {
    const history = buildHistory({
      messages: [message('u1', 'User', 'hi', {}, 1)],
    });
    const items = placeRuns(history, {
      runs: [emptyLiveRun(runFixture('run_a', { status: 'running' }))],
    });

    expect(items[0]).toBe(history.items[0]);
    expect(history.items).toHaveLength(1);
  });

  it('keeps a finished run it saw stream until its messages arrive', () => {
    const done = runFixture('run_1', { status: 'completed', startedAtMs: 1 });
    const live = {
      ...emptyLiveRun(done),
      steps: [{ stepId: 'run_1:1', text: 'Streamed', textOffset: 0 }],
    };

    expect(kinds(buildTranscript({ messages: [], runs: [live] }))).toEqual([
      'run',
    ]);
    expect(
      kinds(
        buildTranscript({
          messages: [
            message('a1', 'Assistant', 'Streamed', { runId: 'run_1' }),
          ],
          runs: [live],
        }),
      ),
    ).toEqual(['message']);
  });

  it('labels a stopped run only through its committed partial reply', () => {
    const stopped = runFixture('run_s', {
      status: 'cancelled',
      startedAtMs: 1,
    });
    const items = buildTranscript({
      messages: [
        message('u1', 'User', 'go', { runId: 'run_s' }),
        message('a1', 'Assistant', 'Half', { runId: 'run_s', stopped: true }),
      ],
      runs: [emptyLiveRun(stopped)],
    });

    expect(kinds(items)).toEqual(['message', 'message']);
  });
});

describe('tool steps', () => {
  it('names helpers from their arguments and results', () => {
    const items = buildTranscript({
      messages: [
        message('a1', 'Assistant', '', {
          runId: 'run_1',
          toolCalls: [
            {
              id: 'h1',
              name: 'spawn_helper',
              args: { name: 'Researcher', task: 'find vendors' },
            },
            {
              id: 'd1',
              name: 'delegate_to_agent',
              args: { agent_id: 'agent-ops', task: 'book' },
            },
          ],
        }),
        message('t1', 'Tool', '{"agentId":"helper-7","status":"success"}', {
          runId: 'run_1',
          toolCallId: 'h1',
        }),
      ],
    });
    const block = items[0];

    expect(
      block.kind === 'tools' && block.steps.map((step) => step.helper),
    ).toEqual([
      { label: 'Researcher', agentId: 'helper-7' },
      { label: 'agent-ops', agentId: 'agent-ops' },
    ]);
  });

  it('reads live cards the way it reads stored calls', () => {
    const steps = liveToolSteps({
      ...emptyLiveRun(runFixture('run_1', { status: 'running' })),
      tools: [
        {
          stepId: 'run_1:1',
          toolCallId: 'call_1',
          name: 'web_search',
          argumentsPreview: '{"query":"cheap flights"}',
          argumentsTruncated: false,
          status: 'running',
          durationMs: null,
          resultPreview: null,
          truncated: false,
        },
      ],
    });

    expect(steps).toEqual([
      expect.objectContaining({
        name: 'web_search',
        argumentsPreview: 'query: cheap flights',
        status: 'running',
        result: null,
        runId: 'run_1',
      }),
    ]);
  });

  it('summarizes arguments on one short line', () => {
    expect(argumentsSummary({ path: 'notes.md', lines: [1, 2] })).toBe(
      'path: notes.md, lines: [1,2]',
    );
    expect(argumentsSummary({})).toBe('');
    expect(argumentsSummary({ text: 'x'.repeat(200) })).toHaveLength(120);
    expect(previewSummary('{"query": "cut off')).toBe('{"query": "cut off');
    expect(delegatedTaskText('plain task')).toBe('plain task');
    expect(formatElapsed(400)).toBe('<1s');
    expect(formatElapsed(4_400)).toBe('4s');
  });
});

describe('mergeSessionRuns', () => {
  it('prefers the stream’s view and takes a finish only the ledger saw', () => {
    const running = runFixture('run_1', { status: 'running', createdAtMs: 2 });
    const live = {
      ...emptyLiveRun(running),
      steps: [{ stepId: 'run_1:1', text: 'Hi', textOffset: 0 }],
    };
    const finished = { ...running, status: 'failed' as const };
    const other = runFixture('run_0', {
      status: 'interrupted',
      createdAtMs: 1,
    });

    const merged = mergeSessionRuns([live], [finished, other]);

    expect(merged.map((item) => item.run.id)).toEqual(['run_0', 'run_1']);
    expect(merged[1].run.status).toBe('failed');
    expect(merged[1].steps[0].text).toBe('Hi');
    expect(mergeSessionRuns([live], [running])[0]).toBe(live);
  });

  it('prefers the more advanced status when live and ledger are both mid-flight', () => {
    const queued = runFixture('run_1', { status: 'queued', createdAtMs: 1 });
    const staleQueuedLive = emptyLiveRun(queued);
    const ledgerRunning = { ...queued, status: 'running' as const };

    // A stale live `queued` snapshot must not shadow the ledger's `running`
    // (it would otherwise still show "Queued" with a Cancel button).
    expect(
      mergeSessionRuns([staleQueuedLive], [ledgerRunning])[0].run.status,
    ).toBe('running');

    // The reverse never regresses a fresher live status back down.
    const runningLive = emptyLiveRun({ ...queued, status: 'running' });
    expect(mergeSessionRuns([runningLive], [queued])[0].run.status).toBe(
      'running',
    );

    const awaitingLedger = { ...queued, status: 'awaiting_approval' as const };
    expect(
      mergeSessionRuns([runningLive], [awaitingLedger])[0].run.status,
    ).toBe('awaiting_approval');
  });
});
