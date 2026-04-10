import { describe, expect, it } from 'vitest';
import { buildDisplayedAgents } from './displayed-agents.js';
import type { AgentEntry, AgentProfile } from './types.js';

function createAgentProfile(
  overrides: Partial<AgentProfile> = {}
): AgentProfile {
  return {
    name: 'manager',
    role: 'orchestrator',
    ...overrides,
  };
}

function createAgentEntry(overrides: Partial<AgentEntry> = {}): AgentEntry {
  return {
    id: 'launch:manager',
    name: 'manager',
    status: 'idle',
    tokens: 0,
    ...overrides,
  };
}

describe('displayed agent helpers', () => {
  it('returns live agents unchanged when no profiles are configured', () => {
    const liveAgents = [createAgentEntry()];

    expect(buildDisplayedAgents([], liveAgents)).toBe(liveAgents);
  });

  it('keeps configured agents first and fills missing ones with idle placeholders', () => {
    const displayedAgents = buildDisplayedAgents(
      [
        createAgentProfile(),
        createAgentProfile({ name: 'researcher_1', role: 'worker' }),
      ],
      [
        createAgentEntry({
          id: 'launch:researcher_1',
          name: 'researcher_1',
          status: 'thinking',
          tokens: 42,
        }),
      ]
    );

    expect(displayedAgents).toEqual([
      {
        id: 'profile:manager',
        name: 'manager',
        status: 'idle',
        tokens: 0,
      },
      {
        id: 'launch:researcher_1',
        name: 'researcher_1',
        status: 'thinking',
        tokens: 42,
      },
    ]);
  });

  it('appends unconfigured live agents after configured ones', () => {
    const displayedAgents = buildDisplayedAgents(
      [createAgentProfile()],
      [
        createAgentEntry(),
        createAgentEntry({
          id: 'launch:reviewer',
          name: 'reviewer',
          status: 'running_tool',
          tokens: 9,
        }),
      ]
    );

    expect(displayedAgents.map((agent) => agent.name)).toEqual([
      'manager',
      'reviewer',
    ]);
    expect(displayedAgents[1]).toMatchObject({
      id: 'launch:reviewer',
      status: 'running_tool',
      tokens: 9,
    });
  });
});
