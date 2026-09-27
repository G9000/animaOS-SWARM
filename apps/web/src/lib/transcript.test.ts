import { describe, expect, it } from 'vitest';

import { emptyLiveRun } from './session-events';
import {
  argumentsSummary,
  buildTranscript,
  delegatedTaskText,
  formatElapsed,
  liveToolSteps,
  mergeSessionRuns,
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

  // Ruling 3 (web part): an orphan card — a tool result whose call is not
  // in the same block — takes its name from the matching assistant
  // message's `toolCalls` (found anywhere in the loaded history), never
  // from a `toolName` metadata key the daemon does not write.
  it('names an orphan card from the assistant message that made its call', () => {
    const items = buildTranscript({
      messages: [
        message('a0', 'Assistant', '', {
          toolCalls: [{ id: 'call_9', name: 'search', args: {} }],
        }),
        message('u1', 'User', 'hang on'),
        message(
          't0',
          'Tool',
          '{"status":"error","data":null,"error":"timed out"}',
          { toolCallId: 'call_9', toolStatus: 'error' },
        ),
      ],
    });

    expect(kinds(items)).toEqual(['tools', 'message', 'tools']);
    const orphanBlock = items[2];
    expect(orphanBlock.kind === 'tools' && orphanBlock.steps).toEqual([
      expect.objectContaining({
        toolCallId: 'call_9',
        name: 'search',
        status: 'error',
        result: 'timed out',
      }),
    ]);
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
});
