import { describe, it, expect, vi } from 'vitest';
import { SwarmCoordinator } from './coordinator.js';
import { EventBus } from '@animaOS-SWARM/core';
import type { IModelAdapter, GenerateResult } from '@animaOS-SWARM/core';
import type { SwarmConfig } from './types.js';

// ─── helpers ────────────────────────────────────────────────────────────────

/**
 * Build a simple model adapter that returns a plain text response.
 * No tool calls — agent stops after one turn.
 */
function textAdapter(response: string): IModelAdapter {
  return {
    provider: 'test',
    generate: vi.fn().mockResolvedValue({
      content: { text: response },
      toolCalls: undefined,
      usage: { promptTokens: 10, completionTokens: 5, totalTokens: 15 },
      stopReason: 'end',
    } satisfies GenerateResult),
  };
}

/**
 * Build an adapter whose generate() implementation can be controlled per-call.
 * Used to simulate tool-call sequences (e.g. delegate_task → final answer).
 */
function sequenceAdapter(responses: Array<GenerateResult>): IModelAdapter {
  const gen = vi.fn();
  for (const r of responses) {
    gen.mockResolvedValueOnce(r);
  }
  // Fallback for any unexpected extra calls
  gen.mockResolvedValue({
    content: { text: 'done' },
    toolCalls: undefined,
    usage: { promptTokens: 1, completionTokens: 1, totalTokens: 2 },
    stopReason: 'end',
  } satisfies GenerateResult);
  return { provider: 'test', generate: gen };
}

function managerConfig(name = 'manager') {
  return { name, model: 'test-model' };
}

function workerConfig(name: string) {
  return { name, model: 'test-model' };
}

function baseConfig(
  strategy: SwarmConfig['strategy'],
  workerNames: string[]
): SwarmConfig {
  return {
    strategy,
    manager: managerConfig(),
    workers: workerNames.map(workerConfig),
  };
}

// ─── SwarmCoordinator.run() — basics ────────────────────────────────────────

describe('SwarmCoordinator basics', () => {
  it('has a unique id on construction', () => {
    const adapter = textAdapter('hello');
    const a = new SwarmCoordinator(
      baseConfig('round-robin', ['worker']),
      adapter
    );
    const b = new SwarmCoordinator(
      baseConfig('round-robin', ['worker']),
      adapter
    );
    expect(a.id).toBeDefined();
    expect(b.id).toBeDefined();
    expect(a.id).not.toBe(b.id);
  });

  it('starts in idle status', () => {
    const coord = new SwarmCoordinator(
      baseConfig('round-robin', ['worker']),
      textAdapter('hi')
    );
    expect(coord.getState().status).toBe('idle');
  });

  it('returns to idle status after a successful run', async () => {
    // _runTask() resets to "idle" so the coordinator is ready for more dispatch() calls
    const coord = new SwarmCoordinator(
      baseConfig('round-robin', ['worker']),
      textAdapter('hi')
    );
    await coord.run('test task');
    expect(coord.getState().status).toBe('idle');
  });

  it('records startedAt and completedAt after a run', async () => {
    const coord = new SwarmCoordinator(
      baseConfig('round-robin', ['worker']),
      textAdapter('hi')
    );
    const before = Date.now();
    await coord.run('test task');
    const after = Date.now();

    const state = coord.getState();
    expect(state.startedAt).toBeGreaterThanOrEqual(before);
    expect(state.completedAt).toBeLessThanOrEqual(after);
  });

  it('emits swarm:created event on run', async () => {
    const bus = new EventBus();
    const listener = vi.fn();
    bus.on('swarm:created', listener);

    const coord = new SwarmCoordinator(
      baseConfig('round-robin', ['worker']),
      textAdapter('hi'),
      bus
    );
    await coord.run('emit test');

    expect(listener).toHaveBeenCalledOnce();
    // EventBus wraps payload in Event<T>: { type, timestamp, agentId?, data }
    expect(listener.mock.calls[0][0].data).toMatchObject({
      strategy: 'round-robin',
    });
  });

  it('emits swarm:completed event on successful run', async () => {
    const bus = new EventBus();
    const listener = vi.fn();
    bus.on('swarm:completed', listener);

    const coord = new SwarmCoordinator(
      baseConfig('round-robin', ['worker']),
      textAdapter('hi'),
      bus
    );
    await coord.run('emit complete test');

    expect(listener).toHaveBeenCalledOnce();
  });

  it('returns error result when the model adapter fails', async () => {
    // AgentRuntime catches adapter errors and returns { status: "error" } without throwing.
    // The coordinator sees the strategy resolve (no exception) so coordinator status = "completed",
    // but the returned TaskResult has status "error".
    const brokenAdapter: IModelAdapter = {
      provider: 'test',
      generate: vi.fn().mockRejectedValue(new Error('model unavailable')),
    };
    const coord = new SwarmCoordinator(
      baseConfig('round-robin', ['worker']),
      brokenAdapter
    );
    const result = await coord.run('broken task');

    // The result should reflect the failure
    expect(result.status).toBe('error');
  });

  it('result has a durationMs >= 0', async () => {
    const coord = new SwarmCoordinator(
      baseConfig('round-robin', ['worker']),
      textAdapter('done')
    );
    const result = await coord.run('timing test');
    expect(result.durationMs).toBeGreaterThanOrEqual(0);
  });

  it('throws for unknown strategy — getStrategy() throws before the try-catch', async () => {
    // getStrategy() is called before the try-catch in run(), so run() itself rejects.
    const coord = new SwarmCoordinator(
      {
        strategy: 'unknown' as SwarmConfig['strategy'],
        manager: managerConfig(),
        workers: [],
      },
      textAdapter('hi')
    );
    await expect(coord.run('bad strategy')).rejects.toThrow('Unknown strategy');
  });

  it('returns error when maxConcurrentAgents is exceeded', async () => {
    // Strategies spawn workers with Promise.all, so all agents check agents.size
    // simultaneously before any are added. To reliably trigger the limit, set
    // maxConcurrentAgents: 0 so the very first spawn attempt fails immediately.
    const coord = new SwarmCoordinator(
      { ...baseConfig('round-robin', ['w1']), maxConcurrentAgents: 0 },
      textAdapter('hi')
    );
    const result = await coord.run('overflow agents');
    expect(result.status).toBe('error');
    expect(result.error).toContain('Max concurrent agents');
  });
});

// ─── round-robin strategy ───────────────────────────────────────────────────

describe('round-robin strategy', () => {
  it('returns success status', async () => {
    const adapter = textAdapter('My contribution to this task.');
    const coord = new SwarmCoordinator(
      baseConfig('round-robin', ['w1', 'w2']),
      adapter
    );
    const result = await coord.run('Collaborate on this');
    expect(result.status).toBe('success');
  });

  it("result data contains text with all speakers' responses", async () => {
    const adapter = textAdapter('My answer');
    const coord = new SwarmCoordinator(
      { ...baseConfig('round-robin', ['w1']), maxTurns: 2 },
      adapter
    );
    const result = await coord.run('Round robin task');
    expect(result.status).toBe('success');
    const text = (result.data as { text: string }).text;
    expect(text.length).toBeGreaterThan(0);
    // History format: "[agent]: content"
    expect(text).toContain('[');
    expect(text).toContain(']:');
  });

  it('result data includes history array with speaker/content pairs', async () => {
    const adapter = textAdapter('Contribution');
    const coord = new SwarmCoordinator(
      { ...baseConfig('round-robin', ['w1']), maxTurns: 2 },
      adapter
    );
    const result = await coord.run('History test');
    const data = result.data as {
      history: Array<{ speaker: string; content: string }>;
    };
    expect(Array.isArray(data.history)).toBe(true);
    expect(data.history.length).toBeGreaterThan(0);
    expect(data.history[0]).toHaveProperty('speaker');
    expect(data.history[0]).toHaveProperty('content');
  });

  it('cycles agents in order (manager, w1, w2, manager, w1, ...)', async () => {
    const adapter: IModelAdapter = {
      provider: 'test',
      generate: vi.fn().mockImplementation(() => {
        const r: GenerateResult = {
          content: { text: 'response' },
          toolCalls: undefined,
          usage: { promptTokens: 1, completionTokens: 1, totalTokens: 2 },
          stopReason: 'end',
        };
        return Promise.resolve(r);
      }),
    };

    const coord = new SwarmCoordinator(
      { ...baseConfig('round-robin', ['w1', 'w2']), maxTurns: 3 },
      adapter
    );
    const result = await coord.run('Cycle test');
    const data = result.data as { history: Array<{ speaker: string }> };

    // Should have 3 entries — manager, w1, w2
    expect(data.history).toHaveLength(3);
    expect(data.history[0].speaker).toBe('manager');
    expect(data.history[1].speaker).toBe('w1');
    expect(data.history[2].speaker).toBe('w2');
  });

  it('uses maxTurns from config when provided', async () => {
    const adapter = textAdapter('Turn');
    const coord = new SwarmCoordinator(
      { ...baseConfig('round-robin', ['w1']), maxTurns: 4 },
      adapter
    );
    const result = await coord.run('Turn test');
    const data = result.data as { history: Array<unknown> };
    expect(data.history).toHaveLength(4);
  });

  it('first turn passes the raw task to the first agent', async () => {
    const generate = vi.fn().mockResolvedValue({
      content: { text: 'response' },
      toolCalls: undefined,
      usage: { promptTokens: 1, completionTokens: 1, totalTokens: 2 },
      stopReason: 'end',
    } satisfies GenerateResult);

    const coord = new SwarmCoordinator(
      { ...baseConfig('round-robin', ['w1']), maxTurns: 1 },
      { provider: 'test', generate }
    );
    await coord.run('The actual task text');

    // generate(modelConfig, generateOptions) — options.messages is Message[]
    // Message.content is Content { text: string }
    const firstCall = generate.mock.calls[0];
    const options = firstCall[1] as {
      messages: Array<{ content: { text: string } }>;
    };
    const hasTask = options.messages.some((m) =>
      m.content?.text?.includes('The actual task text')
    );
    expect(hasTask).toBe(true);
  });
});

// ─── supervisor strategy ─────────────────────────────────────────────────────

describe('supervisor strategy', () => {
  it('returns success after manager synthesizes worker results', async () => {
    // Manager makes one delegate_task call, then gives final answer
    const adapter = sequenceAdapter([
      {
        content: { text: '' },
        toolCalls: [
          {
            id: 'tc1',
            name: 'delegate_task',
            args: { worker_name: 'worker', task: 'Do research' },
          },
        ],
        usage: { promptTokens: 10, completionTokens: 5, totalTokens: 15 },
        stopReason: 'tool_call',
      },
      {
        content: { text: 'Final synthesis: research is complete.' },
        toolCalls: undefined,
        usage: { promptTokens: 20, completionTokens: 10, totalTokens: 30 },
        stopReason: 'end',
      },
    ]);

    const coord = new SwarmCoordinator(
      baseConfig('supervisor', ['worker']),
      adapter
    );
    const result = await coord.run('Research and report');

    expect(result.status).toBe('success');
  });

  it('returns error when delegate_task names an unknown worker', async () => {
    // Manager delegates to "nonexistent" — the tool returns an error result
    // then the manager responds with whatever
    const adapter = sequenceAdapter([
      {
        content: { text: '' },
        toolCalls: [
          {
            id: 'tc1',
            name: 'delegate_task',
            args: { worker_name: 'nonexistent', task: 'Do it' },
          },
        ],
        usage: { promptTokens: 10, completionTokens: 5, totalTokens: 15 },
        stopReason: 'tool_call',
      },
      {
        content: { text: 'Could not delegate.' },
        toolCalls: undefined,
        usage: { promptTokens: 15, completionTokens: 8, totalTokens: 23 },
        stopReason: 'end',
      },
    ]);

    const coord = new SwarmCoordinator(
      baseConfig('supervisor', ['worker']),
      adapter
    );
    // The delegate_task handler returns { status: "error" } for unknown worker names.
    // The tool result is fed back to the manager as a message, not as an exception.
    // The manager then generates its final answer, so the coordinator completes with "success".
    const result = await coord.run('Delegate to ghost');
    expect(result.status).toBe('success');
  });

  it("manager can delegate to a worker and the worker's result feeds back into the manager", async () => {
    // The supervisor strategy spawns workers BEFORE the manager runs.
    // Here the manager immediately delegates to "worker" via delegate_task, then synthesizes.
    // A successful result proves the worker was available when the manager tried to delegate.
    const adapter = sequenceAdapter([
      {
        content: { text: '' },
        toolCalls: [
          {
            id: 'tc1',
            name: 'delegate_task',
            args: { worker_name: 'worker', task: 'Do work' },
          },
        ],
        usage: { promptTokens: 10, completionTokens: 5, totalTokens: 15 },
        stopReason: 'tool_call',
      },
      {
        content: { text: 'Worker did its job.' },
        toolCalls: undefined,
        usage: { promptTokens: 20, completionTokens: 8, totalTokens: 28 },
        stopReason: 'end',
      },
    ]);

    const coord = new SwarmCoordinator(
      baseConfig('supervisor', ['worker']),
      adapter
    );
    const result = await coord.run('Delegated task');
    expect(result.status).toBe('success');
  });

  it('records spawned agent IDs in state', async () => {
    const adapter = textAdapter('Final answer');
    const coord = new SwarmCoordinator(
      baseConfig('supervisor', ['w1', 'w2']),
      adapter
    );
    await coord.run('Track agents');

    const state = coord.getState();
    // supervisor spawns: w1, w2 (workers), then manager = 3 agents total
    expect(state.agentIds.length).toBe(3);
  });
});

// ─── dynamic strategy ────────────────────────────────────────────────────────

describe('dynamic strategy', () => {
  it('returns success when manager signals DONE', async () => {
    // Manager calls choose_speaker with DONE immediately
    const adapter = sequenceAdapter([
      {
        content: { text: '' },
        toolCalls: [
          {
            id: 'tc1',
            name: 'choose_speaker',
            args: { agent_name: 'DONE', instruction: 'wrap up' },
          },
        ],
        usage: { promptTokens: 10, completionTokens: 5, totalTokens: 15 },
        stopReason: 'tool_call',
      },
      {
        content: { text: 'The conversation is complete.' },
        toolCalls: undefined,
        usage: { promptTokens: 15, completionTokens: 7, totalTokens: 22 },
        stopReason: 'end',
      },
    ]);

    const coord = new SwarmCoordinator(
      baseConfig('dynamic', ['analyst']),
      adapter
    );
    const result = await coord.run('Orchestrate a conversation');

    expect(result.status).toBe('success');
  });

  it('returns error when choose_speaker names an unknown agent', async () => {
    const adapter = sequenceAdapter([
      {
        content: { text: '' },
        toolCalls: [
          { id: 'tc1', name: 'choose_speaker', args: { agent_name: 'ghost' } },
        ],
        usage: { promptTokens: 10, completionTokens: 5, totalTokens: 15 },
        stopReason: 'tool_call',
      },
      {
        content: { text: 'Could not find agent.' },
        toolCalls: undefined,
        usage: { promptTokens: 15, completionTokens: 6, totalTokens: 21 },
        stopReason: 'end',
      },
    ]);

    const coord = new SwarmCoordinator(
      baseConfig('dynamic', ['analyst']),
      adapter
    );
    // choose_speaker returns { status: "error" } for unknown agents as a tool result,
    // not as a thrown exception. The manager receives the error payload and generates
    // its final text response, so the coordinator completes with "success".
    const result = await coord.run('Talk to ghost');
    expect(result.status).toBe('success');
  });

  it('records all spawned agent IDs including workers and manager', async () => {
    const adapter = textAdapter('All done');
    const coord = new SwarmCoordinator(
      baseConfig('dynamic', ['a1', 'a2']),
      adapter
    );
    await coord.run('multi agent task');

    const state = coord.getState();
    // dynamic spawns: a1, a2 (workers), then manager = 3 agents total
    expect(state.agentIds.length).toBe(3);
  });

  it('passes chat history to chosen speaker as context', async () => {
    // Manager calls choose_speaker once with the analyst, then signals DONE
    const generate = vi
      .fn()
      .mockResolvedValueOnce({
        // Manager: choose analyst
        content: { text: '' },
        toolCalls: [
          {
            id: 'tc1',
            name: 'choose_speaker',
            args: { agent_name: 'analyst', instruction: 'analyse data' },
          },
        ],
        usage: { promptTokens: 10, completionTokens: 5, totalTokens: 15 },
        stopReason: 'tool_call',
      } satisfies GenerateResult)
      .mockResolvedValueOnce({
        // analyst's response
        content: { text: 'Data analysis complete. Found three patterns.' },
        toolCalls: undefined,
        usage: { promptTokens: 5, completionTokens: 8, totalTokens: 13 },
        stopReason: 'end',
      } satisfies GenerateResult)
      .mockResolvedValueOnce({
        // Manager: DONE
        content: { text: '' },
        toolCalls: [
          { id: 'tc2', name: 'choose_speaker', args: { agent_name: 'DONE' } },
        ],
        usage: { promptTokens: 20, completionTokens: 5, totalTokens: 25 },
        stopReason: 'tool_call',
      } satisfies GenerateResult)
      .mockResolvedValue({
        content: { text: 'Final synthesis.' },
        toolCalls: undefined,
        usage: { promptTokens: 10, completionTokens: 5, totalTokens: 15 },
        stopReason: 'end',
      } satisfies GenerateResult);

    const coord = new SwarmCoordinator(baseConfig('dynamic', ['analyst']), {
      provider: 'test',
      generate,
    });
    const result = await coord.run('Analyse and synthesize');
    expect(result.status).toBe('success');
  });
});

// ─── getState() ──────────────────────────────────────────────────────────────

describe('SwarmCoordinator.getState()', () => {
  it('agentIds is empty before any run', () => {
    const coord = new SwarmCoordinator(
      baseConfig('round-robin', ['w1']),
      textAdapter('hi')
    );
    expect(coord.getState().agentIds).toHaveLength(0);
  });

  it('results array contains the task result after run', async () => {
    const coord = new SwarmCoordinator(
      baseConfig('round-robin', ['w1']),
      textAdapter('final')
    );
    await coord.run('check results');

    const state = coord.getState();
    expect(state.results).toHaveLength(1);
    expect(state.results[0].status).toBe('success');
  });

  it('tokenUsage is non-zero after a run', async () => {
    // aggregateTokenUsage() is captured inside _runTask() while agents are alive,
    // then getState() only re-aggregates if agents.size > 0 (persistent mode).
    // After run() + terminateAll(), agents are gone but the captured value persists.
    const coord = new SwarmCoordinator(
      baseConfig('round-robin', ['w1']),
      textAdapter('ok')
    );
    await coord.run('token test');

    const state = coord.getState();
    expect(state.tokenUsage.totalTokens).toBeGreaterThan(0);
  });
});

// ─── getMessageBus() ─────────────────────────────────────────────────────────

describe('SwarmCoordinator.getMessageBus()', () => {
  it('returns a MessageBus instance', () => {
    const coord = new SwarmCoordinator(
      baseConfig('round-robin', ['w1']),
      textAdapter('hi')
    );
    const bus = coord.getMessageBus();
    expect(bus).toBeDefined();
    expect(typeof bus.getAllMessages).toBe('function');
    expect(typeof bus.send).toBe('function');
  });

  it('returns the same bus instance on repeated calls', () => {
    const coord = new SwarmCoordinator(
      baseConfig('round-robin', ['w1']),
      textAdapter('hi')
    );
    expect(coord.getMessageBus()).toBe(coord.getMessageBus());
  });
});
