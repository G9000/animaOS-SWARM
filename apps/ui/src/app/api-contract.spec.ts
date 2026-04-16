import { describe, expect, it } from 'vitest';

import {
  normalizeAgentCreateResponse,
  normalizeAgentListResponse,
  normalizeAgentRunResponse,
  normalizeSwarmCreateResponse,
  normalizeSwarmListResponse,
  normalizeSwarmRunResponse,
} from './api-contract';

describe('api contract normalizers', () => {
  it('passes through flat server agent list payloads', () => {
    expect(
      normalizeAgentListResponse({
        agents: [
          {
            id: 'agent-1',
            name: 'observer',
            status: 'idle',
          },
        ],
      })
    ).toEqual([
      {
        id: 'agent-1',
        name: 'observer',
        status: 'idle',
        tokenUsage: undefined,
      },
    ]);
  });

  it('maps rust daemon runtime snapshots into flat dashboard agents', () => {
    expect(
      normalizeAgentListResponse({
        agents: [
          {
            state: {
              id: 'agent-2',
              name: 'writer',
              status: 'completed',
              tokenUsage: {
                totalTokens: 12,
              },
            },
            messageCount: 4,
          },
        ],
      })
    ).toEqual([
      {
        id: 'agent-2',
        name: 'writer',
        status: 'completed',
        tokenUsage: {
          totalTokens: 12,
        },
      },
    ]);
  });

  it('maps rust daemon create and run responses to the UI task contract', () => {
    expect(
      normalizeAgentCreateResponse({
        agent: {
          state: {
            id: 'agent-3',
            name: 'operator',
            status: 'idle',
          },
        },
      })
    ).toEqual({
      id: 'agent-3',
      name: 'operator',
      status: 'idle',
    });

    expect(
      normalizeAgentRunResponse({
        agent: {
          state: {
            id: 'agent-3',
          },
        },
        result: {
          status: 'success',
          data: {
            text: 'done',
          },
          durationMs: 42,
        },
      })
    ).toEqual({
      status: 'success',
      data: {
        text: 'done',
      },
      durationMs: 42,
    });
  });

  it('passes through flat swarm payloads and supports rust create/run shapes', () => {
    expect(
      normalizeSwarmListResponse({
        swarms: [
          {
            id: 'swarm-1',
            status: 'idle',
            agentIds: ['agent-1'],
          },
        ],
      })
    ).toEqual([
      {
        id: 'swarm-1',
        status: 'idle',
        agentIds: ['agent-1'],
      },
    ]);

    expect(
      normalizeSwarmCreateResponse({
        swarm: {
          id: 'swarm-2',
          status: 'idle',
        },
      })
    ).toEqual({
      id: 'swarm-2',
      strategy: undefined,
    });

    expect(
      normalizeSwarmRunResponse({
        swarm: {
          id: 'swarm-2',
        },
        result: {
          status: 'error',
          error: 'provider unavailable',
          durationMs: 7,
        },
      })
    ).toEqual({
      status: 'error',
      error: 'provider unavailable',
      durationMs: 7,
    });
  });
});
