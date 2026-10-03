import type { Skill, SkillDraft } from '@animaOS-SWARM/sdk';

export function skillFixture(
  slug: string,
  overrides: Partial<Skill> = {},
): Skill {
  return {
    slug,
    name: slug,
    description: `About ${slug}`,
    enabled: true,
    status: 'active',
    approvedHash: 'a'.repeat(64),
    approvedAtMs: 1,
    updatedAtMs: 1,
    ...overrides,
  };
}

export function skillDraftFixture(
  id: string,
  overrides: Partial<SkillDraft> = {},
): SkillDraft {
  return {
    id,
    slug: 'notes',
    name: 'Notes',
    description: 'Take notes',
    body: 'Write it down.',
    source: 'agent',
    proposedBy: { agentId: 'agent-main', sessionId: 'chat:1', runId: 'run_1' },
    baseHash: null,
    currentHash: null,
    stale: false,
    fileHash: null,
    createdAtMs: 1,
    status: 'pending',
    decidedAtMs: null,
    problem: null,
    ...overrides,
  };
}
