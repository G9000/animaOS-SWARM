export { AgentsClient, action, agent, plugin } from './agents.js';
export type { AgentMemory, AgentRunResponse, AgentSnapshot } from './agents.js';

export { DaemonClient, DaemonHttpError, createDaemonClient } from './client.js';
export type { DaemonClientOptions, DaemonEvent, FetchLike } from './client.js';

export { SwarmsClient, swarm } from './swarms.js';
export type {
  SwarmAgentEventPayload,
  SwarmAgentTokensPayload,
  SwarmEventPayload,
  SwarmStreamEventPayload,
  SwarmTaskFailedPayload,
  SwarmToolAfterPayload,
  SwarmToolBeforePayload,
  SwarmRunResponse,
} from './swarms.js';

export type {
  Action,
  AgentConfig,
  AgentSettings,
  AgentState,
  AgentStatus,
  Attachment,
  Content,
  Plugin,
  TaskResult,
  TokenUsage,
  UUID,
} from '@animaOS-SWARM/core';

export type {
  AgentMessage,
  SwarmConfig,
  SwarmState,
  SwarmStrategy,
} from '@animaOS-SWARM/swarm';
