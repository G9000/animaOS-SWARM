import React from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { IEventBus, TaskResult } from '@animaOS-SWARM/core';

vi.mock('./hooks/use-event-log.js', () => ({
  useEventLog: vi.fn(),
}));

import { App } from './app.js';
import { useEventLog, type UseEventLogResult } from './hooks/use-event-log.js';
import {
  cleanupInk,
  flushInk as flush,
  pressInkKey as pressKey,
  renderInk as render,
  submitInk as submit,
} from './test-harness.js';
import type { ResultEntry } from './components/result-log.js';
import type { AgentProfile } from './types.js';

afterEach(() => {
  cleanupInk();
  vi.clearAllMocks();
  vi.useRealTimers();
});

function createEventBus(): IEventBus {
  return {
    on() {
      return () => undefined;
    },
    async emit() {},
    clear() {},
  };
}

function createResultEntry(overrides: Partial<ResultEntry> = {}): ResultEntry {
  return {
    id: 'run-1',
    timestamp: 1,
    task: 'Earlier task',
    result: 'Earlier result',
    isError: false,
    ...overrides,
  };
}

function createAgentProfile(
  overrides: Partial<AgentProfile> = {}
): AgentProfile {
  return {
    name: 'manager',
    role: 'orchestrator',
    bio: 'Keeps the swarm aligned.',
    adjectives: ['sharp', 'methodical'],
    topics: ['coordination', 'trace'],
    ...overrides,
  };
}

function mockEventLog(overrides: Partial<UseEventLogResult> = {}) {
  vi.mocked(useEventLog).mockReturnValue({
    agents: [],
    messages: [],
    tools: [],
    stats: {
      totalTokens: 0,
      totalCost: 0,
      elapsed: 0,
      agentCount: 0,
      strategy: 'round-robin',
    },
    done: false,
    result: null,
    error: null,
    reset: vi.fn(),
    ...overrides,
  });
}

function createDeferredResult() {
  let resolve!: (value: TaskResult<{ text: string }>) => void;
  const promise = new Promise<TaskResult<{ text: string }>>((res) => {
    resolve = res;
  });

  return { promise, resolve };
}

describe('App interactions', () => {
  it('starts in resumed result view and returns to swarm with /back', async () => {
    mockEventLog();

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="round-robin"
        interactive
        initialResults={[createResultEntry()]}
        resumeLastResult
      />
    );

    expect(rendered.lastFrame()).toContain('Resumed last run.');
    expect(rendered.lastFrame()).toContain('Earlier result');

    await submit(rendered, '/back');

    expect(rendered.lastFrame()).toContain('SWARM');
    expect(rendered.lastFrame()).toContain('Past runs');
  });

  it('retries the resumed task from result view', async () => {
    mockEventLog();
    const onTask = vi
      .fn<(task: string) => Promise<TaskResult<{ text: string }>>>()
      .mockResolvedValue({
        status: 'success',
        data: { text: 'Retried result' },
        durationMs: 5,
      });
    const onResultRecorded = vi.fn();

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="round-robin"
        interactive
        onTask={onTask}
        onResultRecorded={onResultRecorded}
        initialResults={[createResultEntry()]}
        resumeLastResult
      />
    );

    await submit(rendered, '/retry');

    expect(onTask).toHaveBeenCalledWith('Earlier task');
    expect(onResultRecorded).toHaveBeenCalledWith(
      expect.objectContaining({
        task: 'Earlier task',
        result: 'Retried result',
        isError: false,
      })
    );
    expect(rendered.lastFrame()).toContain('SWARM');
    expect(rendered.lastFrame()).toContain('Retried result');
  });

  it('opens trace from slash commands and returns with q', async () => {
    mockEventLog({
      messages: [
        {
          id: 'msg-1',
          from: 'user',
          to: 'manager',
          content: 'Inspect trace',
          timestamp: 1,
        },
      ],
      tools: [
        {
          id: 'tool-1',
          agentId: 'launch:manager',
          agentName: 'manager',
          toolName: 'memory_search',
          args: { query: 'trace' },
          status: 'success',
          result: 'Trace result',
          durationMs: 4,
          timestamp: 2,
        },
      ],
    });

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="round-robin"
        interactive
        initialResults={[createResultEntry()]}
      />
    );

    await submit(rendered, '/trace');

    expect(rendered.lastFrame()).toContain('Trace');
    expect(rendered.lastFrame()).toContain('Tool call');

    await pressKey(rendered, 'q');

    expect(rendered.lastFrame()).toContain('SWARM');
  });

  it('opens history and retries the selected run', async () => {
    mockEventLog();
    const onTask = vi
      .fn<(task: string) => Promise<TaskResult<{ text: string }>>>()
      .mockResolvedValue({
        status: 'success',
        data: { text: 'History retry result' },
        durationMs: 6,
      });

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="round-robin"
        interactive
        onTask={onTask}
        initialResults={[
          createResultEntry({
            id: 'run-1',
            task: 'First task',
            result: 'First result',
          }),
          createResultEntry({
            id: 'run-2',
            timestamp: 2,
            task: 'Second task',
            result: 'Second result',
          }),
        ]}
      />
    );

    await submit(rendered, '/history');

    expect(rendered.lastFrame()).toContain('History');
    expect(rendered.lastFrame()).toContain('Second task');

    await pressKey(rendered, 'r');

    expect(onTask).toHaveBeenCalledWith('Second task');
    expect(rendered.lastFrame()).toContain('SWARM');
    expect(rendered.lastFrame()).toContain('History retry result');
  });

  it('clears resumed session history with /clear', async () => {
    const reset = vi.fn();
    const onClearHistory = vi.fn();
    mockEventLog({ reset });

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="round-robin"
        interactive
        initialResults={[createResultEntry()]}
        resumeLastResult
        onClearHistory={onClearHistory}
      />
    );

    expect(rendered.lastFrame()).toContain('Resumed last run.');

    await submit(rendered, '/clear');

    expect(reset).toHaveBeenCalledOnce();
    expect(onClearHistory).toHaveBeenCalledOnce();
    expect(rendered.lastFrame()).toContain('SWARM');
    expect(rendered.lastFrame()).toContain('Session cleared.');
    expect(rendered.lastFrame()).toContain('interactive');
    expect(rendered.lastFrame()).not.toContain('Past runs');
  });

  it('opens one-shot history with h and returns with q', async () => {
    mockEventLog({
      done: true,
      result: 'One-shot result',
    });

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="round-robin"
        task="Ship the patch"
        initialResults={[createResultEntry()]}
      />
    );

    expect(rendered.lastFrame()).toContain('h history');

    await pressKey(rendered, 'h');

    expect(rendered.lastFrame()).toContain('History');
    expect(rendered.lastFrame()).toContain('Earlier task');

    await pressKey(rendered, 'q');

    expect(rendered.lastFrame()).toContain('SWARM');
  });

  it('opens one-shot trace with t and returns with q', async () => {
    mockEventLog({
      done: true,
      result: 'One-shot result',
      messages: [
        {
          id: 'msg-1',
          from: 'user',
          to: 'manager',
          content: 'Inspect trace',
          timestamp: 1,
        },
      ],
      tools: [
        {
          id: 'tool-1',
          agentId: 'launch:manager',
          agentName: 'manager',
          toolName: 'memory_search',
          args: { query: 'trace' },
          status: 'success',
          result: 'Trace result',
          durationMs: 4,
          timestamp: 2,
        },
      ],
    });

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="round-robin"
        task="Inspect trace"
      />
    );

    expect(rendered.lastFrame()).toContain('t trace');

    await pressKey(rendered, 't');

    expect(rendered.lastFrame()).toContain('Trace');
    expect(rendered.lastFrame()).toContain('Tool call');

    await pressKey(rendered, 'q');

    expect(rendered.lastFrame()).toContain('SWARM');
  });

  it('retries the current one-shot task with r and records it in history', async () => {
    mockEventLog({
      done: true,
      result: 'One-shot result',
    });
    const onTask = vi
      .fn<(task: string) => Promise<TaskResult<{ text: string }>>>()
      .mockResolvedValue({
        status: 'success',
        data: { text: 'Retried one-shot result' },
        durationMs: 7,
      });
    const onResultRecorded = vi.fn();

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="round-robin"
        task="Ship the patch"
        onTask={onTask}
        onResultRecorded={onResultRecorded}
        initialResults={[
          createResultEntry({
            id: 'run-0',
            task: 'Earlier task',
            result: 'Earlier result',
          }),
        ]}
      />
    );

    expect(rendered.lastFrame()).toContain('r retry');

    await pressKey(rendered, 'r');

    expect(onTask).toHaveBeenCalledWith('Ship the patch');
    expect(onResultRecorded).toHaveBeenCalledWith(
      expect.objectContaining({
        task: 'Ship the patch',
        result: 'Retried one-shot result',
        isError: false,
      })
    );

    await pressKey(rendered, 'h');

    expect(rendered.lastFrame()).toContain('History');
    expect(rendered.lastFrame()).toContain('Ship the patch');
    expect(rendered.lastFrame()).toContain('Retried one-shot result');
  });

  it('shows slash command help in the swarm view', async () => {
    mockEventLog();

    const rendered = render(
      <App eventBus={createEventBus()} strategy="round-robin" interactive />
    );

    await submit(rendered, '/help');

    expect(rendered.lastFrame()).toContain('/agents  browse and edit agents');
    expect(rendered.lastFrame()).toContain('/delete');
    expect(rendered.lastFrame()).toContain('/rename');
    expect(rendered.lastFrame()).toContain('/resume');
    expect(rendered.lastFrame()).toContain('/undo');
    expect(rendered.lastFrame()).toContain('/undo-drop');
    expect(rendered.lastFrame()).toContain('/undo-status');
    expect(rendered.lastFrame()).toContain('resume by label');
    expect(rendered.lastFrame()).toContain('/clear');
    expect(rendered.lastFrame()).toContain('session history');
  });

  it('resumes a saved run directly by label', async () => {
    mockEventLog();

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="round-robin"
        interactive
        initialResults={[
          createResultEntry({
            id: 'run-1',
            task: 'First task',
            result: 'First result',
            label: 'launch hotfix',
          }),
          createResultEntry({
            id: 'run-2',
            timestamp: 2,
            task: 'Second task',
            result: 'Second result',
            label: 'docs sweep',
          }),
        ]}
      />
    );

    await submit(rendered, '/resume hotfix');

    expect(rendered.lastFrame()).toContain('Result');
    expect(rendered.lastFrame()).toContain('Saved run:');
    expect(rendered.lastFrame()).toContain('launch hotfix');
    expect(rendered.lastFrame()).toContain('First task');
    expect(rendered.lastFrame()).toContain('Resumed saved run.');
  });

  it('shows matching saved-run labels when /resume label is ambiguous', async () => {
    mockEventLog();

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="round-robin"
        interactive
        initialResults={[
          createResultEntry({
            id: 'run-1',
            task: 'First task',
            result: 'First result',
            label: 'launch hotfix',
            timestamp: 1,
          }),
          createResultEntry({
            id: 'run-2',
            timestamp: 2,
            task: 'Second task',
            result: 'Second result',
            label: 'launch docs',
          }),
        ]}
      />
    );

    await submit(rendered, '/resume launch');

    expect(rendered.lastFrame()).toContain(
      'Multiple saved runs match "launch"'
    );
    expect(rendered.lastFrame()).toContain('Saved run matches for "launch"');
    expect(rendered.lastFrame()).toContain('"launch docs"');
    expect(rendered.lastFrame()).toContain('"launch hotfix"');
    expect(rendered.lastFrame()).toContain('Ctrl+Y to open');
    expect(rendered.lastFrame()).toContain('SWARM');
  });

  it('shows close saved-run suggestions after a failed /resume label lookup', async () => {
    mockEventLog();

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="round-robin"
        interactive
        initialResults={[
          createResultEntry({
            id: 'run-1',
            task: 'First task',
            result: 'First result',
            label: 'launch hotfix',
          }),
          createResultEntry({
            id: 'run-2',
            timestamp: 2,
            task: 'Second task',
            result: 'Second result',
            label: 'docs sweep',
          }),
        ]}
      />
    );

    await submit(rendered, '/resume hotfx');

    expect(rendered.lastFrame()).toContain(
      'No saved run named "hotfx". Type /resume to browse saved runs.'
    );
    expect(rendered.lastFrame()).toContain('Closest saved runs for "hotfx"');
    expect(rendered.lastFrame()).toContain('launch hotfix');
    expect(rendered.lastFrame()).toContain('First task');
  });

  it('opens a saved-run suggestion directly from the swarm assist panel', async () => {
    mockEventLog();

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="round-robin"
        interactive
        initialResults={[
          createResultEntry({
            id: 'run-1',
            task: 'First task',
            result: 'First result',
            label: 'launch hotfix',
            timestamp: 1,
          }),
          createResultEntry({
            id: 'run-2',
            timestamp: 2,
            task: 'Second task',
            result: 'Second result',
            label: 'launch docs',
          }),
        ]}
      />
    );

    await submit(rendered, '/resume launch');

    expect(rendered.lastFrame()).toContain('Saved run matches for "launch"');
    expect(rendered.lastFrame()).toContain('❯ launch docs');

    await pressKey(rendered, '\u0010');

    expect(rendered.lastFrame()).toContain('❯ launch hotfix');

    await pressKey(rendered, '\u0019');

    expect(rendered.lastFrame()).toContain('Result');
    expect(rendered.lastFrame()).toContain('launch hotfix');
    expect(rendered.lastFrame()).toContain('First task');
    expect(rendered.lastFrame()).toContain('Resumed saved run.');
  });

  it('shows inline /resume label completions before submitting the command', async () => {
    mockEventLog();

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="round-robin"
        interactive
        initialResults={[
          createResultEntry({
            id: 'run-1',
            task: 'First task',
            result: 'First result',
            label: 'launch hotfix',
            timestamp: 1,
          }),
          createResultEntry({
            id: 'run-2',
            timestamp: 2,
            task: 'Second task',
            result: 'Second result',
            label: 'launch docs',
          }),
        ]}
      />
    );

    for (const char of '/resume ') {
      await pressKey(rendered, char);
    }

    expect(rendered.lastFrame()).toContain('launch docs');
    expect(rendered.lastFrame()).toContain('launch hotfix');
    expect(rendered.lastFrame()).toContain('Second task');

    await pressKey(rendered, '\u001B[B');
    await pressKey(rendered, '\t');

    expect(rendered.lastFrame()).toContain('/resume launch hotfix');
    expect(rendered.lastFrame()).toContain('launch hotfix');

    await pressKey(rendered, '\r');

    expect(rendered.lastFrame()).toContain('Result');
    expect(rendered.lastFrame()).toContain('launch hotfix');
    expect(rendered.lastFrame()).toContain('First task');
    expect(rendered.lastFrame()).toContain('Resumed saved run.');
  });

  it('suggests a suffixed /rename label completion when the typed label is already taken', async () => {
    mockEventLog();
    const onHistoryUpdated = vi.fn();

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="round-robin"
        interactive
        onHistoryUpdated={onHistoryUpdated}
        initialResults={[
          createResultEntry({
            id: 'run-1',
            task: 'First task',
            result: 'First result',
            label: 'launch hotfix',
            timestamp: 1,
          }),
          createResultEntry({
            id: 'run-2',
            timestamp: 2,
            task: 'Second task',
            result: 'Second result',
          }),
        ]}
      />
    );

    for (const char of '/rename launch hotfix') {
      await pressKey(rendered, char);
    }

    expect(rendered.lastFrame()).toContain('launch hotfix 2');
    expect(rendered.lastFrame()).toContain('next available label');

    await pressKey(rendered, '\t');

    expect(rendered.lastFrame()).toContain('/rename launch hotfix 2');

    await pressKey(rendered, '\r');

    expect(rendered.lastFrame()).toContain('Saved run named: launch hotfix 2');
    expect(onHistoryUpdated).toHaveBeenCalledWith(
      expect.arrayContaining([
        expect.objectContaining({
          id: 'run-2',
          label: 'launch hotfix 2',
        }),
      ])
    );
  });

  it('warns when /rename reuses an existing saved-run label', async () => {
    mockEventLog();
    const onHistoryUpdated = vi.fn();

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="round-robin"
        interactive
        onHistoryUpdated={onHistoryUpdated}
        initialResults={[
          createResultEntry({
            id: 'run-1',
            task: 'First task',
            result: 'First result',
            label: 'launch hotfix',
            timestamp: 1,
          }),
          createResultEntry({
            id: 'run-2',
            timestamp: 2,
            task: 'Second task',
            result: 'Second result',
          }),
        ]}
      />
    );

    await submit(rendered, '/rename launch hotfix');

    expect(rendered.lastFrame()).toContain(
      'Saved run label "launch hotfix" is already used.'
    );
    expect(rendered.lastFrame()).toContain('Try /rename launch hotfix 2');
    expect(onHistoryUpdated).not.toHaveBeenCalled();
  });

  it('renames the selected saved run and shows the label in the resume picker', async () => {
    mockEventLog();
    const onHistoryUpdated = vi.fn();

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="round-robin"
        interactive
        onHistoryUpdated={onHistoryUpdated}
        initialResults={[
          createResultEntry({
            id: 'run-1',
            task: 'First task',
            result: 'First result',
          }),
          createResultEntry({
            id: 'run-2',
            timestamp: 2,
            task: 'Second task',
            result: 'Second result',
          }),
        ]}
      />
    );

    await submit(rendered, '/resume');
    await pressKey(rendered, '\u001B[B');
    await pressKey(rendered, '\r');

    await submit(rendered, '/rename launch hotfix');

    expect(rendered.lastFrame()).toContain('Saved run named: launch hotfix');
    expect(rendered.lastFrame()).toContain('Saved run:');
    expect(rendered.lastFrame()).toContain('launch hotfix');
    expect(onHistoryUpdated).toHaveBeenCalledWith(
      expect.arrayContaining([
        expect.objectContaining({
          id: 'run-1',
          label: 'launch hotfix',
        }),
      ])
    );

    await submit(rendered, '/resume');

    expect(rendered.lastFrame()).toContain('launch hotfix');
    expect(rendered.lastFrame()).toContain('First task');
  });

  it('prioritizes named saved runs when opening the resume picker', async () => {
    mockEventLog();

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="round-robin"
        interactive
        initialResults={[
          createResultEntry({
            id: 'run-1',
            timestamp: 1,
            task: 'Named task',
            result: 'Named result',
            label: 'release prep',
          }),
          createResultEntry({
            id: 'run-2',
            timestamp: 2,
            task: 'Unnamed latest task',
            result: 'Unnamed latest result',
          }),
        ]}
      />
    );

    await submit(rendered, '/resume');

    expect(rendered.lastFrame()).toContain('Resume');
    expect(rendered.lastFrame()).toContain('Saved run:');
    expect(rendered.lastFrame()).toContain('release prep');
    expect(rendered.lastFrame()).toContain('Named task');
    expect(rendered.lastFrame()).not.toContain('Unnamed latest result');
  });

  it('shows a helpful message when /resume label does not match a saved run', async () => {
    mockEventLog();

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="round-robin"
        interactive
        initialResults={[
          createResultEntry({
            id: 'run-1',
            task: 'First task',
            result: 'First result',
            label: 'launch hotfix',
          }),
        ]}
      />
    );

    await submit(rendered, '/resume missing-run');

    expect(rendered.lastFrame()).toContain(
      'No saved run named "missing-run". Type /resume to browse saved runs.'
    );
    expect(rendered.lastFrame()).toContain('SWARM');
  });

  it('opens the resume picker and retries the selected saved run', async () => {
    mockEventLog();
    const onTask = vi
      .fn<(task: string) => Promise<TaskResult<{ text: string }>>>()
      .mockResolvedValue({
        status: 'success',
        data: { text: 'Retried saved run' },
        durationMs: 5,
      });

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="round-robin"
        interactive
        onTask={onTask}
        initialResults={[
          createResultEntry({
            id: 'run-1',
            task: 'First task',
            result: 'First result',
          }),
          createResultEntry({
            id: 'run-2',
            timestamp: 2,
            task: 'Second task',
            result: 'Second result',
          }),
        ]}
      />
    );

    await submit(rendered, '/resume');

    expect(rendered.lastFrame()).toContain('Resume');
    expect(rendered.lastFrame()).toContain('enter resume');

    await pressKey(rendered, '\u001B[B');
    await pressKey(rendered, '\r');

    expect(rendered.lastFrame()).toContain('Result');
    expect(rendered.lastFrame()).toContain('First task');
    expect(rendered.lastFrame()).toContain('First result');
    expect(rendered.lastFrame()).toContain('Resumed saved run.');

    await submit(rendered, '/back');
    await submit(rendered, '/result');

    expect(rendered.lastFrame()).toContain('Second task');
    expect(rendered.lastFrame()).toContain('Second result');
    expect(rendered.lastFrame()).not.toContain('Resumed saved run.');

    await submit(rendered, '/resume');
    await pressKey(rendered, '\u001B[B');
    await pressKey(rendered, '\r');
    await submit(rendered, '/retry');

    expect(onTask).toHaveBeenCalledWith('First task');
    expect(rendered.lastFrame()).toContain('Retried saved run');
  });

  it('deletes the selected saved run from the resume picker and persists the history update', async () => {
    mockEventLog();
    const onHistoryUpdated = vi.fn();

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="round-robin"
        interactive
        onHistoryUpdated={onHistoryUpdated}
        initialResults={[
          createResultEntry({
            id: 'run-1',
            timestamp: 1,
            task: 'Named task',
            result: 'Named result',
            label: 'release prep',
          }),
          createResultEntry({
            id: 'run-2',
            timestamp: 2,
            task: 'Latest task',
            result: 'Latest result',
          }),
        ]}
      />
    );

    await submit(rendered, '/resume');

    expect(rendered.lastFrame()).toContain('Resume');
    expect(rendered.lastFrame()).toContain('x delete');
    expect(rendered.lastFrame()).toContain('release prep');

    await pressKey(rendered, 'x');

    expect(onHistoryUpdated).not.toHaveBeenCalled();
    expect(rendered.lastFrame()).toContain('x confirm delete');
    expect(rendered.lastFrame()).toContain('Confirm delete');

    await pressKey(rendered, 'x');

    expect(rendered.lastFrame()).toContain('Deleted saved run: release prep');
    expect(rendered.lastFrame()).toContain('Saved runs (1)');
    expect(rendered.lastFrame()).toContain('Latest task');
    expect(onHistoryUpdated).toHaveBeenCalledWith([
      expect.objectContaining({
        id: 'run-2',
        task: 'Latest task',
      }),
    ]);

    expect(rendered.lastFrame()).toContain('u undo delete');
    expect(rendered.lastFrame()).toContain('Press u to restore release prep.');

    await pressKey(rendered, 'u');

    expect(rendered.lastFrame()).toContain('Restored saved run: release prep');
    expect(rendered.lastFrame()).toContain('Saved runs (2)');
    expect(rendered.lastFrame()).toContain('release prep');
    expect(onHistoryUpdated).toHaveBeenLastCalledWith([
      expect.objectContaining({
        id: 'run-1',
        label: 'release prep',
      }),
      expect.objectContaining({
        id: 'run-2',
        task: 'Latest task',
      }),
    ]);
  });

  it('undoes multiple saved-run deletions in LIFO order from the resume picker', async () => {
    mockEventLog();
    const onHistoryUpdated = vi.fn();

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="round-robin"
        interactive
        onHistoryUpdated={onHistoryUpdated}
        initialResults={[
          createResultEntry({
            id: 'run-1',
            timestamp: 1,
            task: 'Alpha task',
            result: 'Alpha result',
            label: 'alpha',
          }),
          createResultEntry({
            id: 'run-2',
            timestamp: 2,
            task: 'Bravo task',
            result: 'Bravo result',
            label: 'bravo',
          }),
          createResultEntry({
            id: 'run-3',
            timestamp: 3,
            task: 'Charlie task',
            result: 'Charlie result',
            label: 'charlie',
          }),
        ]}
      />
    );

    await submit(rendered, '/resume');

    expect(rendered.lastFrame()).toContain('charlie');

    await pressKey(rendered, 'x');
    await pressKey(rendered, 'x');

    expect(rendered.lastFrame()).toContain('Deleted saved run: charlie');
    expect(rendered.lastFrame()).toContain('bravo');
    expect(rendered.lastFrame()).not.toContain('charlie  Charlie task');

    await pressKey(rendered, 'x');
    await pressKey(rendered, 'x');

    expect(rendered.lastFrame()).toContain('Deleted saved run: bravo');
    expect(rendered.lastFrame()).toContain('Press u to restore bravo.');
    expect(rendered.lastFrame()).toContain('1 more deleted saved run queued.');
    expect(rendered.lastFrame()).toContain('Saved runs (1)');

    await pressKey(rendered, 'u');

    expect(rendered.lastFrame()).toContain(
      'Restored saved run: bravo. 1 more deleted run queued for undo.'
    );
    expect(rendered.lastFrame()).toContain('Press u to restore charlie.');
    expect(rendered.lastFrame()).toContain('Saved runs (2)');

    await pressKey(rendered, 'u');

    expect(rendered.lastFrame()).toContain('Restored saved run: charlie');
    expect(rendered.lastFrame()).toContain('Saved runs (3)');
    expect(onHistoryUpdated).toHaveBeenLastCalledWith([
      expect.objectContaining({
        id: 'run-1',
        label: 'alpha',
      }),
      expect.objectContaining({
        id: 'run-2',
        label: 'bravo',
      }),
      expect.objectContaining({
        id: 'run-3',
        label: 'charlie',
      }),
    ]);
  });

  it('drops the oldest queued undo when the delete stack exceeds its limit', async () => {
    mockEventLog();
    const onHistoryUpdated = vi.fn();

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="round-robin"
        interactive
        onHistoryUpdated={onHistoryUpdated}
        initialResults={[
          createResultEntry({
            id: 'run-1',
            timestamp: 1,
            task: 'One task',
            result: 'One result',
            label: 'one',
          }),
          createResultEntry({
            id: 'run-2',
            timestamp: 2,
            task: 'Two task',
            result: 'Two result',
            label: 'two',
          }),
          createResultEntry({
            id: 'run-3',
            timestamp: 3,
            task: 'Three task',
            result: 'Three result',
            label: 'three',
          }),
          createResultEntry({
            id: 'run-4',
            timestamp: 4,
            task: 'Four task',
            result: 'Four result',
            label: 'four',
          }),
          createResultEntry({
            id: 'run-5',
            timestamp: 5,
            task: 'Five task',
            result: 'Five result',
            label: 'five',
          }),
          createResultEntry({
            id: 'run-6',
            timestamp: 6,
            task: 'Six task',
            result: 'Six result',
            label: 'six',
          }),
        ]}
      />
    );

    await submit(rendered, '/resume');

    for (let index = 0; index < 6; index += 1) {
      await pressKey(rendered, 'x');
      await pressKey(rendered, 'x');
    }

    expect(rendered.lastFrame()).toContain('Deleted saved run: one');
    expect(rendered.lastFrame()).toContain('Oldest undo dropped: six.');
    expect(rendered.lastFrame()).toContain('Press u to restore one.');
    expect(rendered.lastFrame()).toContain('4 more deleted saved runs queued.');

    for (let index = 0; index < 5; index += 1) {
      await pressKey(rendered, 'u');
    }

    expect(rendered.lastFrame()).toContain('Saved runs (5)');
    expect(rendered.lastFrame()).toContain('five');
    expect(rendered.lastFrame()).not.toContain('six  Six task');
    expect(onHistoryUpdated).toHaveBeenLastCalledWith([
      expect.objectContaining({
        id: 'run-1',
        label: 'one',
      }),
      expect.objectContaining({
        id: 'run-2',
        label: 'two',
      }),
      expect.objectContaining({
        id: 'run-3',
        label: 'three',
      }),
      expect.objectContaining({
        id: 'run-4',
        label: 'four',
      }),
      expect.objectContaining({
        id: 'run-5',
        label: 'five',
      }),
    ]);
  });

  it('shows delete undo queue status without opening the resume picker', async () => {
    mockEventLog();

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="round-robin"
        interactive
        initialResults={[
          createResultEntry({
            id: 'run-1',
            timestamp: 1,
            task: 'Alpha task',
            result: 'Alpha result',
            label: 'alpha',
          }),
          createResultEntry({
            id: 'run-2',
            timestamp: 2,
            task: 'Bravo task',
            result: 'Bravo result',
            label: 'bravo',
          }),
        ]}
      />
    );

    await submit(rendered, '/undo-status');

    expect(rendered.lastFrame()).toContain(
      'Undo queue empty. Delete a saved run first.'
    );

    await submit(rendered, '/delete bravo');
    await submit(rendered, '/delete bravo');
    await submit(rendered, '/delete alpha');
    await submit(rendered, '/delete alpha');
    await submit(rendered, '/undo-status');

    expect(rendered.lastFrame()).toContain('Undo queue: 2 deleted saved runs.');
    expect(rendered.lastFrame()).toContain('Next restore alpha.');
    expect(rendered.lastFrame()).toContain('Oldest queued bravo.');
    expect(rendered.lastFrame()).toContain('Limit 5.');
    expect(rendered.lastFrame()).toContain('Open /resume');
    expect(rendered.lastFrame()).toContain('and press u to restore.');
  });

  it('restores the latest deleted saved run with /undo from swarm view', async () => {
    mockEventLog();
    const onHistoryUpdated = vi.fn();

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="round-robin"
        interactive
        onHistoryUpdated={onHistoryUpdated}
        initialResults={[
          createResultEntry({
            id: 'run-1',
            timestamp: 1,
            task: 'Alpha task',
            result: 'Alpha result',
            label: 'alpha',
          }),
          createResultEntry({
            id: 'run-2',
            timestamp: 2,
            task: 'Bravo task',
            result: 'Bravo result',
            label: 'bravo',
          }),
        ]}
      />
    );

    await submit(rendered, '/undo');

    expect(rendered.lastFrame()).toContain('No deleted saved run to restore.');

    await submit(rendered, '/delete bravo');
    await submit(rendered, '/delete bravo');
    await submit(rendered, '/undo');

    expect(rendered.lastFrame()).toContain('Restored saved run: bravo');
    expect(onHistoryUpdated).toHaveBeenLastCalledWith([
      expect.objectContaining({
        id: 'run-1',
        label: 'alpha',
      }),
      expect.objectContaining({
        id: 'run-2',
        label: 'bravo',
      }),
    ]);
  });

  it('discards the oldest queued undo with /undo-drop from swarm view', async () => {
    mockEventLog();
    const onHistoryUpdated = vi.fn();

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="round-robin"
        interactive
        onHistoryUpdated={onHistoryUpdated}
        initialResults={[
          createResultEntry({
            id: 'run-1',
            timestamp: 1,
            task: 'Alpha task',
            result: 'Alpha result',
            label: 'alpha',
          }),
          createResultEntry({
            id: 'run-2',
            timestamp: 2,
            task: 'Bravo task',
            result: 'Bravo result',
            label: 'bravo',
          }),
        ]}
      />
    );

    await submit(rendered, '/undo-drop');

    expect(rendered.lastFrame()).toContain('No queued undo to discard.');

    await submit(rendered, '/delete bravo');
    await submit(rendered, '/delete bravo');
    await submit(rendered, '/delete alpha');
    await submit(rendered, '/delete alpha');
    await submit(rendered, '/undo-drop');

    expect(rendered.lastFrame()).toContain(
      'Confirm oldest undo discard: repeat /undo-drop to discard bravo.'
    );
    expect(rendered.lastFrame()).toContain('Undo discard armed.');
    expect(rendered.lastFrame()).toContain(
      'Repeat /undo-drop to discard bravo.'
    );
    expect(rendered.lastFrame()).toContain('Any other command cancels.');
    expect(onHistoryUpdated).toHaveBeenCalledTimes(2);

    await submit(rendered, '/undo-drop');

    expect(rendered.lastFrame()).toContain(
      'Dropped oldest queued undo: bravo.'
    );
    expect(rendered.lastFrame()).toContain('1 deleted run still queued.');
    expect(rendered.lastFrame()).toContain('Undo queued: alpha.');
    expect(rendered.lastFrame()).not.toContain('Undo discard armed.');

    await submit(rendered, '/undo');

    expect(rendered.lastFrame()).toContain('Restored saved run: alpha');
    expect(onHistoryUpdated).toHaveBeenLastCalledWith([
      expect.objectContaining({
        id: 'run-1',
        label: 'alpha',
      }),
    ]);
  });

  it('discards the oldest queued undo with D from the resume picker', async () => {
    mockEventLog();

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="round-robin"
        interactive
        initialResults={[
          createResultEntry({
            id: 'run-1',
            timestamp: 1,
            task: 'Alpha task',
            result: 'Alpha result',
            label: 'alpha',
          }),
          createResultEntry({
            id: 'run-2',
            timestamp: 2,
            task: 'Bravo task',
            result: 'Bravo result',
            label: 'bravo',
          }),
          createResultEntry({
            id: 'run-3',
            timestamp: 3,
            task: 'Charlie task',
            result: 'Charlie result',
            label: 'charlie',
          }),
        ]}
      />
    );

    await submit(rendered, '/resume');
    await pressKey(rendered, 'x');
    await pressKey(rendered, 'x');
    await pressKey(rendered, 'x');
    await pressKey(rendered, 'x');

    expect(rendered.lastFrame()).toContain('Press u to restore bravo.');
    expect(rendered.lastFrame()).toContain(
      'Press D to discard oldest queued undo: charlie.'
    );

    await pressKey(rendered, 'D');

    expect(rendered.lastFrame()).toContain('D confirm');
    expect(rendered.lastFrame()).toContain('drop oldest undo');
    expect(rendered.lastFrame()).toContain('Confirm oldest undo discard');
    expect(rendered.lastFrame()).toContain(
      'Press D again to discard oldest queued undo: charlie.'
    );

    await pressKey(rendered, 'D');

    expect(rendered.lastFrame()).toContain(
      'Dropped oldest queued undo: charlie.'
    );
    expect(rendered.lastFrame()).toContain('1 deleted run still queued.');
    expect(rendered.lastFrame()).toContain('Press u to restore bravo.');
    expect(rendered.lastFrame()).not.toContain(
      'Press D to discard oldest queued undo: charlie.'
    );
  });

  it('shows a persistent swarm undo hint when deleted runs are queued', async () => {
    mockEventLog();

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="round-robin"
        interactive
        initialResults={[
          createResultEntry({
            id: 'run-1',
            timestamp: 1,
            task: 'Alpha task',
            result: 'Alpha result',
            label: 'alpha',
          }),
        ]}
      />
    );

    await submit(rendered, '/delete alpha');
    await submit(rendered, '/delete alpha');

    expect(rendered.lastFrame()).toContain('Deleted saved run: alpha');
    expect(rendered.lastFrame()).toContain(
      'Undo queued: alpha. Use /undo or /undo-status.'
    );
    expect(rendered.lastFrame()).not.toContain(
      'Press Ctrl+O to open saved runs.'
    );
  });

  it('shows a full-queue warning in the swarm undo hint when the stack is full', async () => {
    mockEventLog();

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="round-robin"
        interactive
        initialResults={[
          createResultEntry({
            id: 'run-1',
            timestamp: 1,
            task: 'One task',
            result: 'One result',
            label: 'one',
          }),
          createResultEntry({
            id: 'run-2',
            timestamp: 2,
            task: 'Two task',
            result: 'Two result',
            label: 'two',
          }),
          createResultEntry({
            id: 'run-3',
            timestamp: 3,
            task: 'Three task',
            result: 'Three result',
            label: 'three',
          }),
          createResultEntry({
            id: 'run-4',
            timestamp: 4,
            task: 'Four task',
            result: 'Four result',
            label: 'four',
          }),
          createResultEntry({
            id: 'run-5',
            timestamp: 5,
            task: 'Five task',
            result: 'Five result',
            label: 'five',
          }),
        ]}
      />
    );

    await submit(rendered, '/delete five');
    await submit(rendered, '/delete five');
    await submit(rendered, '/delete four');
    await submit(rendered, '/delete four');
    await submit(rendered, '/delete three');
    await submit(rendered, '/delete three');
    await submit(rendered, '/delete two');
    await submit(rendered, '/delete two');
    await submit(rendered, '/delete one');
    await submit(rendered, '/delete one');

    expect(rendered.lastFrame()).toContain('Undo queued: one (+4 more).');
    expect(rendered.lastFrame()).toContain('Queue full.');
    expect(rendered.lastFrame()).toContain('Next delete drops oldest: five.');
    expect(rendered.lastFrame()).toContain('Use /undo or /undo-status.');
  });

  it('deletes the current result with /delete after confirmation', async () => {
    mockEventLog();
    const onHistoryUpdated = vi.fn();

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="round-robin"
        interactive
        resumeLastResult
        onHistoryUpdated={onHistoryUpdated}
        initialResults={[
          createResultEntry({
            id: 'run-1',
            timestamp: 1,
            task: 'Earlier task',
            result: 'Earlier result',
            label: 'release prep',
          }),
          createResultEntry({
            id: 'run-2',
            timestamp: 2,
            task: 'Latest task',
            result: 'Latest result',
            label: 'ship ready',
          }),
        ]}
      />
    );

    expect(rendered.lastFrame()).toContain('ship ready');

    await submit(rendered, '/delete ');

    expect(onHistoryUpdated).not.toHaveBeenCalled();
    expect(rendered.lastFrame()).toContain('Pending delete');
    expect(rendered.lastFrame()).toContain(
      'Repeat /delete to remove ship ready. Any other command cancels.'
    );
    expect(rendered.lastFrame()).toContain(
      'Confirm delete: repeat /delete to remove ship ready.'
    );

    await submit(rendered, '/delete ');

    expect(rendered.lastFrame()).toContain('Deleted saved run: ship ready');
    expect(rendered.lastFrame()).toContain('Earlier result');
    expect(rendered.lastFrame()).not.toContain('Latest result');
    expect(onHistoryUpdated).toHaveBeenCalledWith([
      expect.objectContaining({
        id: 'run-1',
        label: 'release prep',
      }),
    ]);
  });

  it('deletes a saved run by label from the prompt after confirmation', async () => {
    mockEventLog();
    const onHistoryUpdated = vi.fn();

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="round-robin"
        interactive
        onHistoryUpdated={onHistoryUpdated}
        initialResults={[
          createResultEntry({
            id: 'run-1',
            timestamp: 1,
            task: 'Named task',
            result: 'Named result',
            label: 'release prep',
          }),
          createResultEntry({
            id: 'run-2',
            timestamp: 2,
            task: 'Latest task',
            result: 'Latest result',
          }),
        ]}
      />
    );

    for (const char of '/delete ') {
      await pressKey(rendered, char);
    }

    expect(rendered.lastFrame()).toContain('release prep');

    await pressKey(rendered, '\t');
    await pressKey(rendered, '\r');

    expect(onHistoryUpdated).not.toHaveBeenCalled();
    expect(rendered.lastFrame()).toContain(
      'Confirm delete: repeat /delete release prep to remove this saved run.'
    );

    await submit(rendered, '/delete release prep');

    expect(rendered.lastFrame()).toContain('Deleted saved run: release prep');
    expect(rendered.lastFrame()).toContain('Press u to undo from /resume.');
    expect(rendered.lastFrame()).toContain('Latest task');
    expect(rendered.lastFrame()).toContain('Past runs');
    expect(onHistoryUpdated).toHaveBeenCalledWith([
      expect.objectContaining({
        id: 'run-2',
        task: 'Latest task',
      }),
    ]);
  });

  it('asks for a current result or label when /delete is used from swarm view', async () => {
    mockEventLog();

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="round-robin"
        interactive
        initialResults={[
          createResultEntry({
            id: 'run-1',
            timestamp: 1,
            task: 'Named task',
            result: 'Named result',
            label: 'release prep',
          }),
        ]}
      />
    );

    await submit(rendered, '/delete ');

    expect(rendered.lastFrame()).toContain(
      'Open a result or provide a saved run label, for example /delete release prep.'
    );
  });

  it('shows guidance when /delete cannot find an exact saved-run label', async () => {
    mockEventLog();

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="round-robin"
        interactive
        initialResults={[
          createResultEntry({
            id: 'run-1',
            timestamp: 1,
            task: 'Named task',
            result: 'Named result',
            label: 'release prep',
          }),
        ]}
      />
    );

    await submit(rendered, '/delete release');

    expect(rendered.lastFrame()).toContain(
      'No saved run named "release". Use Tab completion or /resume to inspect saved runs.'
    );
    expect(rendered.lastFrame()).toContain('release prep');
  });

  it('opens the resume picker with ctrl+o from the swarm view', async () => {
    mockEventLog();

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="round-robin"
        interactive
        initialResults={[
          createResultEntry({
            id: 'run-1',
            task: 'First task',
            result: 'First result',
          }),
          createResultEntry({
            id: 'run-2',
            timestamp: 2,
            task: 'Second task',
            result: 'Second result',
          }),
        ]}
      />
    );

    expect(rendered.lastFrame()).toContain('Press Ctrl+O to open saved runs.');

    await pressKey(rendered, '\u000f');

    expect(rendered.lastFrame()).toContain('Resume');
    expect(rendered.lastFrame()).toContain('enter resume');
  });

  it('recalls the latest persisted task with up arrow in interactive mode', async () => {
    mockEventLog();
    const onTask = vi
      .fn<(task: string) => Promise<TaskResult<{ text: string }>>>()
      .mockResolvedValue({
        status: 'success',
        data: { text: 'Replayed result' },
        durationMs: 6,
      });

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="round-robin"
        interactive
        onTask={onTask}
        initialResults={[
          createResultEntry({
            id: 'run-1',
            task: 'First task',
            result: 'First result',
          }),
          createResultEntry({
            id: 'run-2',
            timestamp: 2,
            task: 'Second task',
            result: 'Second result',
          }),
        ]}
      />
    );

    expect(rendered.lastFrame()).toContain('↑↓ recall previous prompts');

    await pressKey(rendered, '\u001B[A');

    expect(rendered.lastFrame()).toContain('Second task');

    await pressKey(rendered, '\r');

    expect(onTask).toHaveBeenCalledWith('Second task');
    expect(rendered.lastFrame()).toContain('Replayed result');
  });

  it('shows current session state with /status', async () => {
    mockEventLog({
      messages: [
        {
          id: 'msg-1',
          from: 'user',
          to: 'manager',
          content: 'Inspect trace',
          timestamp: 1,
        },
      ],
      tools: [
        {
          id: 'tool-1',
          agentId: 'launch:manager',
          agentName: 'manager',
          toolName: 'memory_search',
          args: { query: 'trace' },
          status: 'success',
          result: 'Trace result',
          durationMs: 4,
          timestamp: 2,
        },
      ],
      stats: {
        totalTokens: 0,
        totalCost: 0,
        elapsed: 0,
        agentCount: 2,
        strategy: 'round-robin',
      },
    });

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="round-robin"
        interactive
        initialResults={[createResultEntry()]}
      />
    );

    await submit(rendered, '/status');

    expect(rendered.lastFrame()).toContain('Status: waiting');
    expect(rendered.lastFrame()).toContain('task interactive');
    expect(rendered.lastFrame()).toContain('agents 2');
    expect(rendered.lastFrame()).toContain('messages 1');
    expect(rendered.lastFrame()).toContain('tools 1');
    expect(rendered.lastFrame()).toContain('history 1');
    expect(rendered.lastFrame()).toContain('trace ready');
  });

  it('shows configured agents even before live agent events arrive', async () => {
    mockEventLog();

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="supervisor"
        interactive
        agentProfiles={[
          createAgentProfile(),
          createAgentProfile({ name: 'researcher_1', role: 'worker' }),
        ]}
      />
    );

    expect(rendered.lastFrame()).toContain('2 agents (0 active)');
    expect(rendered.lastFrame()).toContain('[manager]');
    expect(rendered.lastFrame()).toContain('[researcher_1]');
  });

  it('shows a persistent daemon preflight warning in the swarm view', async () => {
    mockEventLog();

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="supervisor"
        interactive
        preflightWarning="Failed to reach daemon at http://127.0.0.1:8080. Launch tasks will fail until the daemon is reachable."
      />
    );

    expect(rendered.lastFrame()).toContain('Failed to reach daemon');
    expect(rendered.lastFrame()).toContain('Launch tasks will fail');
    expect(rendered.lastFrame()).toContain('daemon: down');
    expect(rendered.lastFrame()).toContain(
      'daemon down - tasks paused; use /health or /help'
    );
    expect(rendered.lastFrame()).toContain(
      'commands only while daemon is down'
    );

    await submit(rendered, '/help');

    expect(rendered.lastFrame()).toContain('/health');
    expect(rendered.lastFrame()).toContain('daemon connectivity');
    expect(rendered.lastFrame()).not.toContain('/retry  rerun the last task');
  });

  it('hides the past-runs retry footer while the daemon is down', async () => {
    mockEventLog();

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="supervisor"
        interactive
        initialResults={[createResultEntry()]}
        preflightWarning="Failed to reach daemon at http://127.0.0.1:8080. Launch tasks will fail until the daemon is reachable."
      />
    );

    expect(rendered.lastFrame()).toContain('Past runs');
    expect(rendered.lastFrame()).toContain('/history browse all');
    expect(rendered.lastFrame()).not.toContain('/retry rerun last');
  });

  it('removes retry affordances from resumed result view while the daemon is down', async () => {
    mockEventLog();

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="supervisor"
        interactive
        initialResults={[createResultEntry()]}
        resumeLastResult
        preflightWarning="Failed to reach daemon at http://127.0.0.1:8080. Launch tasks will fail until the daemon is reachable."
      />
    );

    expect(rendered.lastFrame()).toContain('Resumed last run.');
    expect(rendered.lastFrame()).toContain(
      'Retry unavailable while daemon is down.'
    );
    expect(rendered.lastFrame()).toContain('Use /health to recheck');

    await submit(rendered, '/help');

    expect(rendered.lastFrame()).toContain('/back  return to swarm view');
    expect(rendered.lastFrame()).toContain('/health');
    expect(rendered.lastFrame()).toContain('daemon connectivity');
    expect(rendered.lastFrame()).not.toContain('/retry  rerun the last task');
  });

  it('blocks interactive task submissions while the daemon is down', async () => {
    mockEventLog();
    const onTask = vi
      .fn<(task: string) => Promise<TaskResult<{ text: string }>>>()
      .mockResolvedValue({
        status: 'success',
        data: { text: 'Should not run' },
        durationMs: 5,
      });

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="supervisor"
        interactive
        onTask={onTask}
        preflightWarning="Failed to reach daemon at http://127.0.0.1:8080. Launch tasks will fail until the daemon is reachable."
      />
    );

    await submit(rendered, 'Draft release notes');

    expect(onTask).not.toHaveBeenCalled();
    expect(rendered.lastFrame()).toContain(
      'Task submission is blocked while the daemon is down.'
    );
    expect(rendered.lastFrame()).toContain('Use /health to recheck.');
  });

  it('blocks slash retries while the daemon is down', async () => {
    mockEventLog();
    const onTask = vi
      .fn<(task: string) => Promise<TaskResult<{ text: string }>>>()
      .mockResolvedValue({
        status: 'success',
        data: { text: 'Should not retry' },
        durationMs: 5,
      });

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="supervisor"
        interactive
        onTask={onTask}
        initialResults={[createResultEntry()]}
        preflightWarning="Failed to reach daemon at http://127.0.0.1:8080. Launch tasks will fail until the daemon is reachable."
      />
    );

    await submit(rendered, '/retry');

    expect(onTask).not.toHaveBeenCalled();
    expect(rendered.lastFrame()).toContain(
      'Task submission is blocked while the daemon is down.'
    );
  });

  it('ignores history retry input while the daemon is down', async () => {
    mockEventLog();
    const onTask = vi
      .fn<(task: string) => Promise<TaskResult<{ text: string }>>>()
      .mockResolvedValue({
        status: 'success',
        data: { text: 'Should not retry from history' },
        durationMs: 5,
      });

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="supervisor"
        interactive
        onTask={onTask}
        initialResults={[
          createResultEntry({
            id: 'run-1',
            task: 'First task',
            result: 'First result',
          }),
          createResultEntry({
            id: 'run-2',
            timestamp: 2,
            task: 'Second task',
            result: 'Second result',
          }),
        ]}
        preflightWarning="Failed to reach daemon at http://127.0.0.1:8080. Launch tasks will fail until the daemon is reachable."
      />
    );

    await submit(rendered, '/history');
    await pressKey(rendered, 'r');

    expect(onTask).not.toHaveBeenCalled();
    expect(rendered.lastFrame()).toContain('History');
    expect(rendered.lastFrame()).not.toContain(
      'Task submission is blocked while the daemon is down.'
    );
  });

  it('hides history retry affordances while the daemon is down', async () => {
    mockEventLog();

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="supervisor"
        interactive
        initialResults={[
          createResultEntry({
            id: 'run-1',
            task: 'First task',
            result: 'First result',
          }),
          createResultEntry({
            id: 'run-2',
            timestamp: 2,
            task: 'Second task',
            result: 'Second result',
          }),
        ]}
        preflightWarning="Failed to reach daemon at http://127.0.0.1:8080. Launch tasks will fail until the daemon is reachable."
      />
    );

    await submit(rendered, '/history');

    expect(rendered.lastFrame()).toContain('History');
    expect(rendered.lastFrame()).not.toContain('r retry');
  });

  it('updates the daemon preflight warning while the app is open', async () => {
    mockEventLog();
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-04-09T12:00:00Z'));

    const pollDaemonWarning = vi
      .fn<() => Promise<string | undefined>>()
      .mockResolvedValueOnce(
        'Failed to reach daemon at http://127.0.0.1:8080. Launch tasks will fail until the daemon is reachable.'
      )
      .mockResolvedValueOnce(undefined);

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="supervisor"
        interactive
        pollDaemonWarning={pollDaemonWarning}
      />
    );

    expect(rendered.lastFrame()).toContain('daemon: up');
    expect(rendered.lastFrame()).toContain('@ 12:00:00Z');
    expect(rendered.lastFrame()).not.toContain('Failed to reach daemon');

    await vi.advanceTimersByTimeAsync(5000);
    await Promise.resolve();
    await Promise.resolve();

    expect(rendered.lastFrame()).toContain('Failed to reach daemon');
    expect(rendered.lastFrame()).toContain('Daemon connection lost');
    expect(rendered.lastFrame()).toContain('daemon: down');
    expect(rendered.lastFrame()).toContain('@ 12:00:05Z');

    await vi.advanceTimersByTimeAsync(5000);
    await Promise.resolve();
    await Promise.resolve();

    expect(rendered.lastFrame()).not.toContain('Failed to reach daemon');
    expect(rendered.lastFrame()).toContain('Daemon reachable again');
    expect(rendered.lastFrame()).toContain('daemon: up');
    expect(rendered.lastFrame()).toContain('@ 12:00:10Z');
    expect(rendered.lastFrame()).toContain(
      'Task entry restored. Freeform tasks are available again.'
    );
    expect(rendered.lastFrame()).toContain(
      'type your task... or /help for commands'
    );

    await vi.advanceTimersByTimeAsync(4000);
    await Promise.resolve();

    expect(rendered.lastFrame()).not.toContain(
      'Task entry restored. Freeform tasks are available again.'
    );
  });

  it('recovers from a daemon-down launch state and then runs a task', async () => {
    mockEventLog();
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-04-09T12:15:00Z'));

    const pollDaemonWarning = vi
      .fn<() => Promise<string | undefined>>()
      .mockResolvedValue(undefined);
    const onTask = vi
      .fn<(task: string) => Promise<TaskResult<{ text: string }>>>()
      .mockResolvedValue({
        status: 'success',
        data: { text: 'Completed after reconnect' },
        durationMs: 7,
      });

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="supervisor"
        interactive
        onTask={onTask}
        preflightWarning="Failed to reach daemon at http://127.0.0.1:8080. Launch tasks will fail until the daemon is reachable."
        pollDaemonWarning={pollDaemonWarning}
      />
    );

    expect(rendered.lastFrame()).toContain('daemon: down');
    expect(rendered.lastFrame()).toContain(
      'daemon down - tasks paused; use /health or /help'
    );

    await vi.advanceTimersByTimeAsync(5000);
    await Promise.resolve();
    await Promise.resolve();

    expect(rendered.lastFrame()).toContain('daemon: up');
    expect(rendered.lastFrame()).toContain(
      'Task entry restored. Freeform tasks are available again.'
    );
    expect(rendered.lastFrame()).toContain(
      'type your task... or /help for commands'
    );
    expect(rendered.lastFrame()).not.toContain(
      'daemon down - tasks paused; use /health or /help'
    );

    vi.useRealTimers();

    await submit(rendered, 'Ship the patch');
    await flush();

    expect(onTask).toHaveBeenCalledWith('Ship the patch');
    expect(rendered.lastFrame()).toContain('Completed after reconnect');
  });

  it('includes the daemon check time in /status when health monitoring is wired', async () => {
    mockEventLog();
    vi.spyOn(Date, 'now').mockReturnValue(
      new Date('2026-04-09T09:45:30Z').valueOf()
    );

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="supervisor"
        interactive
        pollDaemonWarning={vi.fn().mockResolvedValue(undefined)}
      />
    );

    await submit(rendered, '/status');

    expect(rendered.lastFrame()).toContain('daemon up checked 09:45:30Z');
  });

  it('reports daemon health on demand with /health', async () => {
    mockEventLog();

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="supervisor"
        interactive
        pollDaemonWarning={vi.fn().mockResolvedValue(undefined)}
        preflightWarning="Failed to reach daemon at http://127.0.0.1:8080. Launch tasks will fail until the daemon is reachable."
      />
    );

    await submit(rendered, '/health');

    expect(rendered.lastFrame()).toContain('Daemon reachable again.');
    expect(rendered.lastFrame()).toContain('Launch tasks can run.');
    expect(rendered.lastFrame()).toContain(
      'Task entry restored. Freeform tasks are available again.'
    );
    expect(rendered.lastFrame()).toContain(
      'type your task... or /help for commands'
    );
  });

  it('reports unavailable health checks when /health is not wired for the session', async () => {
    mockEventLog();

    const rendered = render(
      <App eventBus={createEventBus()} strategy="supervisor" interactive />
    );

    await submit(rendered, '/health');

    expect(rendered.lastFrame()).toContain(
      'Daemon health checks unavailable in this session.'
    );
  });

  it('shows an informative message when /agents is used without profiles', async () => {
    mockEventLog();

    const rendered = render(
      <App eventBus={createEventBus()} strategy="round-robin" interactive />
    );

    await submit(rendered, '/agents');

    expect(rendered.lastFrame()).toContain(
      'No agent profiles loaded. Create an agency first.'
    );
    expect(rendered.lastFrame()).toContain('SWARM');
  });

  it('opens the agents panel from /agents and returns to swarm with q', async () => {
    mockEventLog();

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="round-robin"
        interactive
        agentProfiles={[
          createAgentProfile(),
          createAgentProfile({
            name: 'writer',
            role: 'worker',
            adjectives: ['concise', 'fast'],
          }),
        ]}
      />
    );

    await submit(rendered, '/agents');

    expect(rendered.lastFrame()).toContain('Agents (2)');
    expect(rendered.lastFrame()).toContain('manager');
    expect(rendered.lastFrame()).toContain('writer');

    await pressKey(rendered, 'q');

    expect(rendered.lastFrame()).toContain('SWARM');
  });

  it('opens the last result from swarm with /result and returns with /back', async () => {
    mockEventLog();

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="round-robin"
        interactive
        initialResults={[
          createResultEntry({
            id: 'run-1',
            task: 'First task',
            result: 'First result',
          }),
          createResultEntry({
            id: 'run-2',
            timestamp: 2,
            task: 'Second task',
            result: 'Second result',
          }),
        ]}
      />
    );

    await submit(rendered, '/result');

    expect(rendered.lastFrame()).toContain('Result');
    expect(rendered.lastFrame()).toContain(
      'Type /rename <label> to name this saved run.'
    );
    expect(rendered.lastFrame()).toContain('Second task');
    expect(rendered.lastFrame()).toContain('Second result');

    await submit(rendered, '/back');

    expect(rendered.lastFrame()).toContain('SWARM');
    expect(rendered.lastFrame()).toContain('Past runs');
  });

  it('shows guard messages for result, history, resume, rename, trace, and retry when nothing is available', async () => {
    mockEventLog();

    const rendered = render(
      <App eventBus={createEventBus()} strategy="round-robin" interactive />
    );

    await submit(rendered, '/result');
    expect(rendered.lastFrame()).toContain('No results yet. Run a task first.');

    await submit(rendered, '/history');
    expect(rendered.lastFrame()).toContain(
      'No run history yet. Run a task first.'
    );

    await submit(rendered, '/resume');
    expect(rendered.lastFrame()).toContain(
      'No saved runs yet. Run a task first.'
    );

    await pressKey(rendered, '\u000f');
    expect(rendered.lastFrame()).toContain(
      'No saved runs yet. Run a task first.'
    );

    await submit(rendered, '/rename sprint prep');
    expect(rendered.lastFrame()).toContain(
      'No saved runs yet. Run a task first.'
    );

    await submit(rendered, '/trace');
    expect(rendered.lastFrame()).toContain('No trace yet. Run a task first.');

    await submit(rendered, '/retry');
    expect(rendered.lastFrame()).toContain('No previous task to retry.');
    expect(rendered.lastFrame()).toContain('SWARM');
  });

  it('shows live session state with s while a task is running', async () => {
    mockEventLog({
      stats: {
        totalTokens: 0,
        totalCost: 0,
        elapsed: 0,
        agentCount: 1,
        strategy: 'round-robin',
      },
    });
    const deferred = createDeferredResult();
    const onTask = vi
      .fn<(task: string) => Promise<TaskResult<{ text: string }>>>()
      .mockReturnValue(deferred.promise);

    const rendered = render(
      <App
        eventBus={createEventBus()}
        strategy="round-robin"
        interactive
        onTask={onTask}
      />
    );

    await submit(rendered, 'Ship the patch');

    expect(rendered.lastFrame()).toContain('running swarm...');
    expect(rendered.lastFrame()).toContain(
      'Press s for status while the swarm is running.'
    );

    await pressKey(rendered, 's');

    expect(rendered.lastFrame()).toContain('Status: running');
    expect(rendered.lastFrame()).toContain('task Ship the patch');
    expect(rendered.lastFrame()).toContain('agents 1');
    expect(rendered.lastFrame()).toContain('messages 0');
    expect(rendered.lastFrame()).toContain('tools 0');
    expect(rendered.lastFrame()).toContain('history');
    expect(rendered.lastFrame()).toContain('trace empty');

    deferred.resolve({
      status: 'success',
      data: { text: 'Completed' },
      durationMs: 5,
    });
    await flush();
  });
});
