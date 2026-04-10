import { describe, expect, it } from 'vitest';

import type { IEventBus } from '@animaOS-SWARM/core';

import {
  emitLaunchTaskFailure,
  emitLaunchTaskQueued,
  emitLaunchTaskStart,
  launchDisplayAgents,
  relayLaunchSwarmEvent,
} from './launch-events.js';

function createRecordingBus(): {
  events: Array<{ type: string; data: unknown; agentId?: string }>;
  bus: IEventBus;
} {
  const events: Array<{ type: string; data: unknown; agentId?: string }> = [];

  return {
    events,
    bus: {
      on() {
        return () => undefined;
      },
      async emit(type, data, agentId) {
        events.push({ type, data, agentId });
      },
      clear() {},
    },
  };
}

describe('launch event bridge', () => {
  const agency = {
    name: 'launch-fixture',
    description: '',
    model: 'gpt-5.4',
    provider: 'openai',
    strategy: 'round-robin' as const,
    orchestrator: {
      name: 'manager',
      bio: 'Coordinate',
      system: 'Coordinate',
    },
    agents: [
      {
        name: 'worker-a',
        bio: 'Work',
        system: 'Work',
      },
    ],
  };

  it('derives stable display agents from an agency config', () => {
    expect(launchDisplayAgents(agency)).toEqual([
      {
        id: 'launch:manager',
        name: 'manager',
        role: 'orchestrator',
      },
      {
        id: 'launch:worker-a',
        name: 'worker-a',
        role: 'worker',
      },
    ]);
  });

  it('emits spawn and task-start events for a launch task', async () => {
    const { bus, events } = createRecordingBus();
    const agents = launchDisplayAgents(agency);

    await emitLaunchTaskStart(bus, agents, 'Ship it');

    expect(events).toEqual([
      {
        type: 'agent:spawned',
        data: { agentId: 'launch:manager', name: 'manager' },
        agentId: 'launch:manager',
      },
      {
        type: 'agent:spawned',
        data: { agentId: 'launch:worker-a', name: 'worker-a' },
        agentId: 'launch:worker-a',
      },
      {
        type: 'agent:message',
        data: {
          from: 'user',
          to: 'manager',
          message: { text: 'Ship it' },
        },
        agentId: 'launch:manager',
      },
      {
        type: 'task:started',
        data: { agentId: 'launch:manager' },
        agentId: 'launch:manager',
      },
    ]);
  });

  it('emits spawn and input context before a launch task starts', async () => {
    const { bus, events } = createRecordingBus();
    const agents = launchDisplayAgents(agency);

    await emitLaunchTaskQueued(bus, agents, 'Ship it');

    expect(events).toEqual([
      {
        type: 'agent:spawned',
        data: { agentId: 'launch:manager', name: 'manager' },
        agentId: 'launch:manager',
      },
      {
        type: 'agent:spawned',
        data: { agentId: 'launch:worker-a', name: 'worker-a' },
        agentId: 'launch:worker-a',
      },
      {
        type: 'agent:message',
        data: {
          from: 'user',
          to: 'manager',
          message: { text: 'Ship it' },
        },
        agentId: 'launch:manager',
      },
    ]);
  });

  it('relays daemon lifecycle events before completion', async () => {
    const { bus, events } = createRecordingBus();
    const agents = launchDisplayAgents(agency);

    await relayLaunchSwarmEvent(bus, agents, {
      event: 'swarm:created',
      data: {
        swarmId: 'swarm-1' as any,
        state: {
          id: 'swarm-1' as any,
          status: 'idle',
          agentIds: [],
          results: [],
          tokenUsage: {
            promptTokens: 0,
            completionTokens: 0,
            totalTokens: 0,
          },
          startedAt: undefined,
          completedAt: undefined,
        },
        result: null,
      },
    });

    await relayLaunchSwarmEvent(bus, agents, {
      event: 'swarm:running',
      data: {
        swarmId: 'swarm-1' as any,
        state: {
          id: 'swarm-1' as any,
          status: 'running',
          agentIds: [],
          results: [],
          tokenUsage: {
            promptTokens: 0,
            completionTokens: 0,
            totalTokens: 5,
          },
          startedAt: undefined,
          completedAt: undefined,
        },
        result: null,
      },
    });

    expect(events).toEqual([
      {
        type: 'swarm:created',
        data: { swarmId: 'swarm-1' },
        agentId: 'launch:manager',
      },
    ]);
  });

  it('relays live worker activity from daemon SSE into the TUI bus', async () => {
    const { bus, events } = createRecordingBus();
    const agents = launchDisplayAgents(agency);

    await relayLaunchSwarmEvent(bus, agents, {
      event: 'task:started',
      data: {
        agentId: 'worker-a-1',
        agentName: 'worker-a',
      },
    });

    await relayLaunchSwarmEvent(bus, agents, {
      event: 'tool:before',
      data: {
        agentId: 'worker-a-1',
        agentName: 'worker-a',
        toolName: 'memory_search',
        args: { query: 'campaign ideas' },
      },
    });

    await relayLaunchSwarmEvent(bus, agents, {
      event: 'agent:tokens',
      data: {
        agentId: 'worker-a-1',
        agentName: 'worker-a',
        usage: {
          promptTokens: 3,
          completionTokens: 2,
          totalTokens: 5,
        },
      },
    });

    await relayLaunchSwarmEvent(bus, agents, {
      event: 'tool:after',
      data: {
        agentId: 'worker-a-1',
        agentName: 'worker-a',
        toolName: 'memory_search',
        status: 'success',
        durationMs: 42,
      },
    });

    await relayLaunchSwarmEvent(bus, agents, {
      event: 'task:completed',
      data: {
        agentId: 'worker-a-1',
        agentName: 'worker-a',
      },
    });

    expect(events).toEqual([
      {
        type: 'task:started',
        data: { agentId: 'launch:worker-a' },
        agentId: 'launch:worker-a',
      },
      {
        type: 'tool:before',
        data: {
          agentId: 'launch:worker-a',
          toolName: 'memory_search',
          args: { query: 'campaign ideas' },
        },
        agentId: 'launch:worker-a',
      },
      {
        type: 'agent:tokens',
        data: {
          agentId: 'launch:worker-a',
          usage: {
            promptTokens: 3,
            completionTokens: 2,
            totalTokens: 5,
          },
        },
        agentId: 'launch:worker-a',
      },
      {
        type: 'tool:after',
        data: {
          agentId: 'launch:worker-a',
          toolName: 'memory_search',
          status: 'success',
          durationMs: 42,
        },
        agentId: 'launch:worker-a',
      },
      {
        type: 'task:completed',
        data: { agentId: 'launch:worker-a' },
        agentId: 'launch:worker-a',
      },
    ]);
  });

  it('relays a completed swarm SSE event into the TUI event bus', async () => {
    const { bus, events } = createRecordingBus();
    const agents = launchDisplayAgents(agency);

    await relayLaunchSwarmEvent(bus, agents, {
      event: 'swarm:completed',
      data: {
        swarmId: 'swarm-1' as any,
        state: {
          id: 'swarm-1' as any,
          status: 'idle',
          agentIds: [],
          results: [],
          tokenUsage: {
            promptTokens: 0,
            completionTokens: 0,
            totalTokens: 5,
          },
          startedAt: undefined,
          completedAt: undefined,
        },
        result: {
          status: 'success',
          data: { text: 'done' },
          durationMs: 11,
        },
      },
    });

    expect(events).toEqual([
      {
        type: 'agent:terminated',
        data: { agentId: 'launch:worker-a' },
        agentId: 'launch:worker-a',
      },
      {
        type: 'agent:message',
        data: { from: 'manager', to: 'user', message: { text: 'done' } },
        agentId: 'launch:manager',
      },
      {
        type: 'task:completed',
        data: {
          agentId: 'launch:manager',
          result: { status: 'success', data: { text: 'done' }, durationMs: 11 },
        },
        agentId: 'launch:manager',
      },
      {
        type: 'swarm:completed',
        data: {
          result: { status: 'success', data: { text: 'done' }, durationMs: 11 },
        },
        agentId: 'launch:manager',
      },
    ]);
  });

  it('emits a synthetic failed completion when daemon launch fails', async () => {
    const { bus, events } = createRecordingBus();
    const agents = launchDisplayAgents(agency);

    await emitLaunchTaskFailure(bus, agents, 'daemon unavailable');

    expect(events).toEqual([
      {
        type: 'task:failed',
        data: { agentId: 'launch:manager', error: 'daemon unavailable' },
        agentId: 'launch:manager',
      },
      {
        type: 'swarm:completed',
        data: {
          result: {
            status: 'error',
            error: 'daemon unavailable',
            durationMs: 0,
          },
        },
        agentId: 'launch:manager',
      },
    ]);
  });
});
