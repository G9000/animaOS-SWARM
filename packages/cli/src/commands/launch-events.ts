import type { IEventBus, TaskResult } from '@animaOS-SWARM/core';
import type {
  DaemonEvent,
  SwarmAgentEventPayload,
  SwarmAgentTokensPayload,
  SwarmEventPayload,
  SwarmStreamEventPayload,
  SwarmTaskFailedPayload,
  SwarmToolAfterPayload,
  SwarmToolBeforePayload,
} from '@animaOS-SWARM/sdk';

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

export async function emitLaunchTaskQueued(
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

  await emitLaunchTaskQueued(eventBus, agents, task);

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
  event: DaemonEvent<SwarmStreamEventPayload>
): Promise<void> {
  const primaryAgent = agents[0];
  if (!primaryAgent) {
    return;
  }

  if (event.event === 'task:started') {
    const data = event.data as SwarmAgentEventPayload;
    const agent = resolveDisplayAgent(agents, data);
    await eventBus.emit(
      'task:started',
      {
        agentId: agent.id,
      },
      agent.id
    );
    return;
  }

  if (event.event === 'task:completed') {
    const data = event.data as SwarmAgentEventPayload;
    const agent = resolveDisplayAgent(agents, data);
    await eventBus.emit(
      'task:completed',
      {
        agentId: agent.id,
      },
      agent.id
    );
    return;
  }

  if (event.event === 'task:failed') {
    const data = event.data as SwarmTaskFailedPayload;
    const agent = resolveDisplayAgent(agents, data);
    await eventBus.emit(
      'task:failed',
      {
        agentId: agent.id,
        error: data.error,
      },
      agent.id
    );
    return;
  }

  if (event.event === 'tool:before') {
    const data = event.data as SwarmToolBeforePayload;
    const agent = resolveDisplayAgent(agents, data);
    await eventBus.emit(
      'tool:before',
      {
        agentId: agent.id,
        toolName: data.toolName,
        args: data.args,
      },
      agent.id
    );
    return;
  }

  if (event.event === 'tool:after') {
    const data = event.data as SwarmToolAfterPayload;
    const agent = resolveDisplayAgent(agents, data);
    await eventBus.emit(
      'tool:after',
      {
        agentId: agent.id,
        toolName: data.toolName,
        status: data.status,
        durationMs: data.durationMs,
        result: data.result,
      },
      agent.id
    );
    return;
  }

  if (event.event === 'agent:tokens') {
    const data = event.data as SwarmAgentTokensPayload;
    const agent = resolveDisplayAgent(agents, data);
    await eventBus.emit(
      'agent:tokens',
      {
        agentId: agent.id,
        usage: data.usage,
      },
      agent.id
    );
    return;
  }

  if (event.event === 'agent:terminated') {
    const data = event.data as SwarmAgentEventPayload;
    const agent = resolveDisplayAgent(agents, data);
    await eventBus.emit(
      'agent:terminated',
      {
        agentId: agent.id,
      },
      agent.id
    );
    return;
  }

  if (event.event === 'swarm:created') {
    const data = event.data as SwarmEventPayload;
    await eventBus.emit(
      'swarm:created',
      {
        swarmId: data.swarmId,
      },
      primaryAgent.id
    );
    return;
  }

  if (event.event === 'swarm:running') {
    return;
  }

  if (event.event !== 'swarm:completed') {
    return;
  }

  const data = event.data as SwarmEventPayload;
  const result = normalizeResult(data.result);

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

function resolveDisplayAgent(
  agents: LaunchDisplayAgent[],
  payload: SwarmAgentEventPayload
): LaunchDisplayAgent {
  return (
    agents.find((agent) => agent.name === payload.agentName) ?? {
      id: payload.agentId,
      name: payload.agentName,
      role: 'worker',
    }
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
