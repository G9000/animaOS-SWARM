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
    expect(rendered.lastFrame()).toContain('/rename');
    expect(rendered.lastFrame()).toContain('/resume');
    expect(rendered.lastFrame()).toContain('resume by label');
    expect(rendered.lastFrame()).toContain('/clear  clear session history');
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
    expect(rendered.lastFrame()).toContain('"launch docs"');
    expect(rendered.lastFrame()).toContain('"launch hotfix"');
    expect(rendered.lastFrame()).toContain('SWARM');
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
    expect(rendered.lastFrame()).toContain('history 0');
    expect(rendered.lastFrame()).toContain('trace empty');

    deferred.resolve({
      status: 'success',
      data: { text: 'Completed' },
      durationMs: 5,
    });
    await flush();
  });
});
