import type {
  AgentRelationship,
  Memory,
  MemoryEntity,
  MemoryFact,
} from '@animaOS-SWARM/sdk';

export function memoryFixture(
  id: string,
  overrides: Partial<Memory> = {},
): Memory {
  return {
    id,
    agentId: 'agent-main',
    agentName: 'Anima',
    type: 'fact',
    content: `Memory ${id}`,
    importance: 0.5,
    createdAt: 1_000,
    scope: 'shared',
    ...overrides,
  };
}

export function factFixture(
  id: string,
  overrides: Partial<MemoryFact> = {},
): MemoryFact {
  return {
    id,
    subjectKind: 'user',
    subjectId: 'owner',
    subjectName: 'You',
    predicate: 'communication_preference',
    objectKind: null,
    objectId: null,
    objectName: null,
    value: 'Prefers short answers',
    validFrom: null,
    validTo: null,
    observedAt: 1_000,
    confidence: 0.9,
    evidenceMemoryIds: [],
    supersedesFactIds: [],
    status: 'active',
    tags: null,
    roomId: null,
    worldId: null,
    sessionId: null,
    createdAt: 1_000,
    updatedAt: 1_000,
    ...overrides,
  };
}

export function entityFixture(
  id: string,
  overrides: Partial<MemoryEntity> = {},
): MemoryEntity {
  return {
    kind: 'external',
    id,
    name: `Entity ${id}`,
    aliases: [],
    summary: null,
    createdAt: 1_000,
    updatedAt: 1_000,
    ...overrides,
  };
}

export function relationshipFixture(
  id: string,
  overrides: Partial<AgentRelationship> = {},
): AgentRelationship {
  return {
    id,
    sourceKind: 'user',
    sourceAgentId: 'owner',
    sourceAgentName: 'You',
    targetKind: 'external',
    targetAgentId: 'entity-1',
    targetAgentName: 'Entity entity-1',
    relationshipType: 'knows',
    strength: 0.5,
    confidence: 0.5,
    evidenceMemoryIds: [],
    createdAt: 1_000,
    updatedAt: 1_000,
    ...overrides,
  };
}
