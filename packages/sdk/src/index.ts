export { AgentsClient, action, agent, plugin } from './agents.js';
export type { AgentMemory, AgentRunResponse, AgentSnapshot } from './agents.js';

export {
  DaemonClient,
  DaemonConnectionError,
  DaemonHttpError,
  createDaemonClient,
} from './client.js';
export type {
  DaemonClientOptions,
  DaemonEvent,
  DaemonHealth,
  FetchLike,
} from './client.js';

export { MemoriesClient } from './memories.js';
export type {
  CreateAgentRelationshipInput,
  CreateMemoryEntityInput,
  CreateMemoryInput,
  EvaluatedMemoryInput,
  MemoryEntity,
  MemoryEntityOptions,
  MemoryEmbeddingStatus,
  MemoryEvidenceTrace,
  MemoryEvalCaseResult,
  MemoryEvalCheckResult,
  MemoryEvalReport,
  MemoryEvaluation,
  MemoryEvaluationDecision,
  MemoryEvaluationOutcome,
  MemoryImportanceAdjustment,
  MemoryRecallOptions,
  MemoryRecallResult,
  MemoryReadiness,
  MemoryRetentionInput,
  MemoryRetentionReport,
  RecentMemoriesOptions,
} from './memories.js';

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
  Memory,
  AgentRelationship,
  AgentRelationshipOptions,
  MemorySearchOptions,
  MemorySearchResult,
  NewAgentRelationshipInput,
  RelationshipEndpointKind,
  MemoryType,
} from '@animaOS-SWARM/memory';

export type {
  AgentMessage,
  SwarmConfig,
  SwarmState,
  SwarmStrategy,
} from '@animaOS-SWARM/swarm';
