import type {
  AgentRelationship,
  AgentRelationshipOptions,
  Memory,
  MemorySearchOptions,
  MemorySearchResult,
  MemoryScope,
  MemoryType,
  NewAgentRelationshipInput,
  RelationshipEndpointKind,
} from '@animaOS-SWARM/memory';

import type { DaemonClient } from './client.js';

export interface CreateMemoryInput {
  agentId: string;
  agentName: string;
  type: MemoryType;
  content: string;
  importance: number;
  tags?: string[] | null;
  scope?: MemoryScope;
  roomId?: string;
  worldId?: string;
  sessionId?: string;
}

export interface RecentMemoriesOptions {
  agentId?: string;
  agentName?: string;
  scope?: MemoryScope;
  roomId?: string;
  worldId?: string;
  sessionId?: string;
  limit?: number;
}

export type CreateAgentRelationshipInput = NewAgentRelationshipInput;

export interface MemoryEntity {
  kind: RelationshipEndpointKind;
  id: string;
  name: string;
  aliases: string[];
  summary?: string | null;
  createdAt: number;
  updatedAt: number;
}

export interface CreateMemoryEntityInput {
  kind: RelationshipEndpointKind;
  id: string;
  name: string;
  aliases?: string[];
  summary?: string;
}

export interface MemoryEntityOptions {
  entityId?: string;
  kind?: RelationshipEndpointKind;
  name?: string;
  alias?: string;
  limit?: number;
}

export type MemoryEvaluationDecision = 'store' | 'merge' | 'ignore';

export interface MemoryEvaluation {
  decision: MemoryEvaluationDecision;
  reason: string;
  score: number;
  suggestedImportance: number;
  duplicateMemoryId?: string | null;
}

export interface EvaluatedMemoryInput extends CreateMemoryInput {
  minContentChars?: number;
  minImportance?: number;
}

export interface MemoryEvaluationOutcome {
  evaluation: MemoryEvaluation;
  memory?: Memory | null;
}

export interface MemoryRecallOptions extends MemorySearchOptions {
  entityId?: string;
  recallAgentId?: string;
  lexicalLimit?: number;
  recentLimit?: number;
  relationshipLimit?: number;
}

export interface MemoryRecallResult {
  memory: Memory;
  score: number;
  lexicalScore: number;
  vectorScore: number;
  relationshipScore: number;
  recencyScore: number;
  importanceScore: number;
}

export interface MemoryEvidenceTrace {
  memory: Memory;
  relationships: AgentRelationship[];
  entities: MemoryEntity[];
}

export interface MemoryImportanceAdjustment {
  memoryId: string;
  previousImportance: number;
  newImportance: number;
}

export interface MemoryRetentionInput {
  maxAgeMillis?: number;
  minImportance?: number;
  maxMemories?: number;
  decayHalfLifeMillis?: number;
}

export interface MemoryRetentionReport {
  decayedMemories: MemoryImportanceAdjustment[];
  removedMemoryIds: string[];
  removedRelationshipIds: string[];
}

export interface MemoryEmbeddingStatus {
  enabled: boolean;
  provider: string;
  model: string;
  dimension: number;
  vectorCount: number;
  persisted: boolean;
  storageFile?: string | null;
}

export interface MemoryEvalCheckResult {
  name: string;
  passed: boolean;
  detail: string;
}

export interface MemoryEvalCaseResult {
  name: string;
  checks: MemoryEvalCheckResult[];
}

export interface MemoryEvalReport {
  passed: boolean;
  totalChecks: number;
  passedChecks: number;
  failureMessages: string[];
  cases: MemoryEvalCaseResult[];
}

export interface MemoryReadiness {
  passed: boolean;
  embeddings: MemoryEmbeddingStatus;
  evaluation: MemoryEvalReport;
}

export class MemoriesClient {
  constructor(private readonly client: DaemonClient) {}

  async create(input: CreateMemoryInput): Promise<Memory> {
    return this.client.requestJson<Memory>('/api/memories', {
      method: 'POST',
      body: input,
    });
  }

  async search(
    query: string,
    options: MemorySearchOptions = {}
  ): Promise<MemorySearchResult[]> {
    const search = new URLSearchParams();
    search.set('q', query);

    if (options.agentId !== undefined) {
      search.set('agentId', options.agentId);
    }
    if (options.agentName !== undefined) {
      search.set('agentName', options.agentName);
    }
    if (options.type !== undefined) {
      search.set('type', options.type);
    }
    if (options.scope !== undefined) {
      search.set('scope', options.scope);
    }
    if (options.roomId !== undefined) {
      search.set('roomId', options.roomId);
    }
    if (options.worldId !== undefined) {
      search.set('worldId', options.worldId);
    }
    if (options.sessionId !== undefined) {
      search.set('sessionId', options.sessionId);
    }
    if (options.limit !== undefined) {
      search.set('limit', String(options.limit));
    }
    if (options.minImportance !== undefined) {
      search.set('minImportance', String(options.minImportance));
    }

    const response = await this.client.requestJson<{
      results: MemorySearchResult[];
    }>(`/api/memories/search?${search.toString()}`);

    return response.results;
  }

  async recent(options: RecentMemoriesOptions = {}): Promise<Memory[]> {
    const search = new URLSearchParams();

    if (options.agentId !== undefined) {
      search.set('agentId', options.agentId);
    }
    if (options.agentName !== undefined) {
      search.set('agentName', options.agentName);
    }
    if (options.scope !== undefined) {
      search.set('scope', options.scope);
    }
    if (options.roomId !== undefined) {
      search.set('roomId', options.roomId);
    }
    if (options.worldId !== undefined) {
      search.set('worldId', options.worldId);
    }
    if (options.sessionId !== undefined) {
      search.set('sessionId', options.sessionId);
    }
    if (options.limit !== undefined) {
      search.set('limit', String(options.limit));
    }

    const path = search.size
      ? `/api/memories/recent?${search.toString()}`
      : '/api/memories/recent';
    const response = await this.client.requestJson<{ memories: Memory[] }>(
      path
    );

    return response.memories;
  }

  async createEntity(input: CreateMemoryEntityInput): Promise<MemoryEntity> {
    return this.client.requestJson<MemoryEntity>('/api/memories/entities', {
      method: 'POST',
      body: input,
    });
  }

  async entities(options: MemoryEntityOptions = {}): Promise<MemoryEntity[]> {
    const search = new URLSearchParams();

    if (options.entityId !== undefined) {
      search.set('entityId', options.entityId);
    }
    if (options.kind !== undefined) {
      search.set('kind', options.kind);
    }
    if (options.name !== undefined) {
      search.set('name', options.name);
    }
    if (options.alias !== undefined) {
      search.set('alias', options.alias);
    }
    if (options.limit !== undefined) {
      search.set('limit', String(options.limit));
    }

    const path = search.size
      ? `/api/memories/entities?${search.toString()}`
      : '/api/memories/entities';
    const response = await this.client.requestJson<{ entities: MemoryEntity[] }>(
      path
    );

    return response.entities;
  }

  async evaluate(input: EvaluatedMemoryInput): Promise<MemoryEvaluation> {
    return this.client.requestJson<MemoryEvaluation>(
      '/api/memories/evaluations',
      {
        method: 'POST',
        body: input,
      }
    );
  }

  async addEvaluated(
    input: EvaluatedMemoryInput
  ): Promise<MemoryEvaluationOutcome> {
    return this.client.requestJson<MemoryEvaluationOutcome>(
      '/api/memories/evaluated',
      {
        method: 'POST',
        body: input,
      }
    );
  }

  async recall(
    query: string,
    options: MemoryRecallOptions = {}
  ): Promise<MemoryRecallResult[]> {
    const search = new URLSearchParams();
    search.set('q', query);

    if (options.agentId !== undefined) {
      search.set('agentId', options.agentId);
    }
    if (options.agentName !== undefined) {
      search.set('agentName', options.agentName);
    }
    if (options.type !== undefined) {
      search.set('type', options.type);
    }
    if (options.scope !== undefined) {
      search.set('scope', options.scope);
    }
    if (options.roomId !== undefined) {
      search.set('roomId', options.roomId);
    }
    if (options.worldId !== undefined) {
      search.set('worldId', options.worldId);
    }
    if (options.sessionId !== undefined) {
      search.set('sessionId', options.sessionId);
    }
    if (options.limit !== undefined) {
      search.set('limit', String(options.limit));
    }
    if (options.minImportance !== undefined) {
      search.set('minImportance', String(options.minImportance));
    }
    if (options.entityId !== undefined) {
      search.set('entityId', options.entityId);
    }
    if (options.recallAgentId !== undefined) {
      search.set('recallAgentId', options.recallAgentId);
    }
    if (options.lexicalLimit !== undefined) {
      search.set('lexicalLimit', String(options.lexicalLimit));
    }
    if (options.recentLimit !== undefined) {
      search.set('recentLimit', String(options.recentLimit));
    }
    if (options.relationshipLimit !== undefined) {
      search.set('relationshipLimit', String(options.relationshipLimit));
    }

    const response = await this.client.requestJson<{
      results: MemoryRecallResult[];
    }>(`/api/memories/recall?${search.toString()}`);

    return response.results;
  }

  async trace(memoryId: string): Promise<MemoryEvidenceTrace> {
    return this.client.requestJson<MemoryEvidenceTrace>(
      `/api/memories/${encodeURIComponent(memoryId)}/trace`
    );
  }

  async applyRetention(
    input: MemoryRetentionInput
  ): Promise<MemoryRetentionReport> {
    return this.client.requestJson<MemoryRetentionReport>(
      '/api/memories/retention',
      {
        method: 'POST',
        body: input,
      }
    );
  }

  async readiness(): Promise<MemoryReadiness> {
    return this.client.requestJson<MemoryReadiness>('/api/memories/readiness');
  }

  async createRelationship(
    input: CreateAgentRelationshipInput
  ): Promise<AgentRelationship> {
    return this.client.requestJson<AgentRelationship>(
      '/api/memories/relationships',
      {
        method: 'POST',
        body: input,
      }
    );
  }

  async relationships(
    options: AgentRelationshipOptions = {}
  ): Promise<AgentRelationship[]> {
    const search = new URLSearchParams();

    if (options.agentId !== undefined) {
      search.set('agentId', options.agentId);
    }
    if (options.entityId !== undefined) {
      search.set('entityId', options.entityId);
    }
    if (options.sourceKind !== undefined) {
      search.set('sourceKind', options.sourceKind);
    }
    if (options.sourceAgentId !== undefined) {
      search.set('sourceAgentId', options.sourceAgentId);
    }
    if (options.targetKind !== undefined) {
      search.set('targetKind', options.targetKind);
    }
    if (options.targetAgentId !== undefined) {
      search.set('targetAgentId', options.targetAgentId);
    }
    if (options.relationshipType !== undefined) {
      search.set('relationshipType', options.relationshipType);
    }
    if (options.roomId !== undefined) {
      search.set('roomId', options.roomId);
    }
    if (options.worldId !== undefined) {
      search.set('worldId', options.worldId);
    }
    if (options.sessionId !== undefined) {
      search.set('sessionId', options.sessionId);
    }
    if (options.minStrength !== undefined) {
      search.set('minStrength', String(options.minStrength));
    }
    if (options.minConfidence !== undefined) {
      search.set('minConfidence', String(options.minConfidence));
    }
    if (options.limit !== undefined) {
      search.set('limit', String(options.limit));
    }

    const path = search.size
      ? `/api/memories/relationships?${search.toString()}`
      : '/api/memories/relationships';
    const response = await this.client.requestJson<{
      relationships: AgentRelationship[];
    }>(path);

    return response.relationships;
  }
}
