import { describe, expect, it } from 'vitest';
import {
  applyLiveEvent,
  buildLiveActivity,
  buildLiveOutputDelta,
  type DashboardSnapshot,
  type LiveEvent,
} from './live-events';

describe('ui live event reducers', () => {
  it('adds spawned agents and keeps health counts aligned', () => {
    const snapshot: DashboardSnapshot = {
      health: {
        status: 'ok',
        agents: 0,
        swarms: 0,
        uptime: 42,
      },
      agents: [],
      swarms: [],
    };

    const next = applyLiveEvent(snapshot, {
      type: 'agent:spawned',
      agentId: 'agent-1',
      timestamp: 100,
      data: {
        agentId: 'agent-1',
        name: 'observer',
      },
    });

    expect(next.health.agents).toBe(1);
    expect(next.agents).toEqual([
      {
        id: 'agent-1',
        name: 'observer',
        status: 'idle',
        tokenUsage: undefined,
      },
    ]);
  });

  it('records swarm completion output for the dashboard', () => {
    const snapshot: DashboardSnapshot = {
      health: {
        status: 'ok',
        agents: 1,
        swarms: 1,
        uptime: 42,
      },
      agents: [
        {
          id: 'agent-1',
          name: 'observer',
          status: 'completed',
        },
      ],
      swarms: [
        {
          id: 'swarm-1',
          status: 'running',
          results: [],
        },
      ],
    };

    const event: LiveEvent = {
      type: 'swarm:completed',
      timestamp: 200,
      data: {
        swarmId: 'swarm-1',
        result: {
          status: 'success',
          data: {
            text: 'rollout ready',
          },
        },
      },
    };

    const next = applyLiveEvent(snapshot, event);
    expect(next.swarms[0]).toMatchObject({
      id: 'swarm-1',
      status: 'idle',
      completedAt: 200,
    });
    expect(next.swarms[0]?.results).toHaveLength(1);
    expect(buildLiveActivity(event)).toMatchObject({
      scope: 'swarms',
      title: 'Live swarm task completed',
      tone: 'success',
    });
    expect(buildLiveOutputDelta(event)).toEqual({
      swarmOutput: {
        id: 'swarm-1',
        body: JSON.stringify(
          {
            status: 'success',
            data: {
              text: 'rollout ready',
            },
          },
          null,
          2
        ),
      },
    });
  });

  it('turns task failure events into agent activity and output text', () => {
    const event: LiveEvent = {
      type: 'task:failed',
      agentId: 'agent-9',
      timestamp: 300,
      data: {
        agentId: 'agent-9',
        error: 'provider unavailable',
      },
    };

    expect(buildLiveActivity(event)).toEqual({
      scope: 'agents',
      title: 'Live agent task failed',
      body: 'provider unavailable',
      tone: 'error',
    });
    expect(buildLiveOutputDelta(event)).toEqual({
      agentOutput: {
        id: 'agent-9',
        body: 'provider unavailable',
      },
    });
  });
});
