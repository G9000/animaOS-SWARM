import type { IEventBus, TaskResult } from '@animaOS-SWARM/core';
import type { DaemonEvent, SwarmEventPayload } from '@animaOS-SWARM/sdk';

import type { AgencyConfig } from '../agency/types.js';
import { extractResultText } from './utils.js';

export interface LaunchDisplayAgent {
  id: string;
  name: string;
  role: 'orchestrator' | 'worker';
}

export function launchDisplayAgents(
  agency: AgencyConfig
): LaunchDisplayAgent[] {
  return [
    {
      id: `launch:${agency.orchestrator.name}`,
      name: agency.orchestrator.name,
      role: 'orchestrator',
    },
    ...agency.agents.map((agent) => ({
      id: `launch:${agent.name}`,
      name: agent.name,
      role: 'worker' as const,
    })),
  ];
}

export async function emitLaunchTaskStart(
  eventBus: IEventBus,
  agents: LaunchDisplayAgent[],
  task: string
): Promise<void> {
  const primaryAgent = agents[0];
  if (!primaryAgent) {
    return;
  }

  for (const agent of agents) {
    await eventBus.emit(
      'agent:spawned',
      {
        agentId: agent.id,
        name: agent.name,
      },
      agent.id
    );
  }

  await eventBus.emit(
    'agent:message',
    {
      from: 'user',
      to: primaryAgent.name,
      message: {
        text: task,
      },
    },
    primaryAgent.id
  );

  await eventBus.emit(
    'task:started',
    {
      agentId: primaryAgent.id,
    },
    primaryAgent.id
  );
}

export async function emitLaunchTaskFailure(
  eventBus: IEventBus,
  agents: LaunchDisplayAgent[],
  error: string
): Promise<void> {
  const primaryAgent = agents[0];
  if (!primaryAgent) {
    return;
  }

  await eventBus.emit(
    'task:failed',
    {
      agentId: primaryAgent.id,
      error,
    },
    primaryAgent.id
  );

  await eventBus.emit(
    'swarm:completed',
    {
      result: {
        status: 'error',
        error,
        durationMs: 0,
      },
    },
    primaryAgent.id
  );
}

export async function relayLaunchSwarmEvent(
  eventBus: IEventBus,
  agents: LaunchDisplayAgent[],
  event: DaemonEvent<SwarmEventPayload>
): Promise<void> {
  const primaryAgent = agents[0];
  if (!primaryAgent) {
    return;
  }

  if (event.event === 'swarm:created') {
    await eventBus.emit(
      'swarm:created',
      {
        swarmId: event.data.swarmId,
      },
      primaryAgent.id
    );
    return;
  }

  if (event.event === 'swarm:running') {
    await emitLaunchAgentTokens(
      eventBus,
      agents,
      event.data.state.tokenUsage.totalTokens
    );
    return;
  }

  if (event.event !== 'swarm:completed') {
    return;
  }

  const result = normalizeResult(event.data.result);
  await emitLaunchAgentTokens(
    eventBus,
    agents,
    event.data.state.tokenUsage.totalTokens
  );

  for (const agent of agents) {
    if (agent.role === 'worker') {
      await eventBus.emit(
        'agent:terminated',
        {
          agentId: agent.id,
        },
        agent.id
      );
    }
  }

  if (result.status === 'success') {
    const text = extractResultText(result);
    if (text) {
      await eventBus.emit(
        'agent:message',
        {
          from: primaryAgent.name,
          to: 'user',
          message: {
            text,
          },
        },
        primaryAgent.id
      );
    }

    await eventBus.emit(
      'task:completed',
      {
        agentId: primaryAgent.id,
        result,
      },
      primaryAgent.id
    );
  } else {
    await eventBus.emit(
      'task:failed',
      {
        agentId: primaryAgent.id,
        error: result.error ?? 'swarm task failed',
      },
      primaryAgent.id
    );
  }

  await eventBus.emit(
    'swarm:completed',
    {
      result,
    },
    primaryAgent.id
  );
}

function normalizeResult(result: SwarmEventPayload['result']): TaskResult {
  if (result) {
    return result;
  }

  return {
    status: 'error',
    error: 'swarm completed without a result',
    durationMs: 0,
  };
}

async function emitLaunchAgentTokens(
  eventBus: IEventBus,
  agents: LaunchDisplayAgent[],
  totalTokens: number
): Promise<void> {
  const tokenShares = distributeTokens(totalTokens, agents.length);

  for (const [index, agent] of agents.entries()) {
    await eventBus.emit(
      'agent:tokens',
      {
        agentId: agent.id,
        usage: {
          totalTokens: tokenShares[index] ?? 0,
        },
      },
      agent.id
    );
  }
}

function distributeTokens(totalTokens: number, count: number): number[] {
  if (count <= 0) {
    return [];
  }

  const base = Math.floor(totalTokens / count);
  let remainder = totalTokens % count;

  return Array.from({ length: count }, () => {
    const next = remainder > 0 ? base + 1 : base;
    if (remainder > 0) {
      remainder -= 1;
    }
    return next;
  });
}
