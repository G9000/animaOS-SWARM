import { describe, expect, it } from 'vitest';

import {
  ACCESS_PROFILES,
  deriveAccessProfile,
  selectMainAgent,
  toolNamesForProfile,
} from './agent-access';
import type { AgentDetail } from './types';

const COMMON_TOOLS = [
  'memory_search',
  'memory_add',
  'recent_memories',
  'get_current_time',
  'calculate',
];

const OBSERVE_TOOLS = [
  ...COMMON_TOOLS,
  'read_file',
  'list_dir',
  'glob',
  'grep',
  'todo_read',
];

const COLLABORATE_TOOLS = [
  ...OBSERVE_TOOLS,
  'write_file',
  'edit_file',
  'multi_edit',
  'todo_write',
];

const OPERATE_TOOLS = [
  ...COLLABORATE_TOOLS,
  'bash',
  'bg_start',
  'bg_output',
  'bg_stop',
  'bg_list',
];

function agent(id: string, createdAtMs: number): AgentDetail {
  return {
    id,
    name: id,
    provider: 'deterministic',
    model: 'deterministic',
    created_at_ms: createdAtMs,
    status: 'Idle',
    token_usage: {
      prompt_tokens: 0,
      completion_tokens: 0,
      total_tokens: 0,
    },
    messages: [],
  };
}

describe('workspace access profiles', () => {
  it('defines presentation metadata for every profile', () => {
    expect(Object.keys(ACCESS_PROFILES)).toEqual([
      'observe',
      'collaborate',
      'operate',
    ]);

    for (const profile of Object.values(ACCESS_PROFILES)) {
      expect(profile.label).not.toHaveLength(0);
      expect(profile.summary).not.toHaveLength(0);
      expect(profile.risk).not.toHaveLength(0);
      expect(profile.tools.length).toBeGreaterThan(0);
    }
  });

  it('returns the exact Observe tools', () => {
    expect(toolNamesForProfile('observe')).toEqual(OBSERVE_TOOLS);
  });

  it('returns the exact Collaborate tools', () => {
    expect(toolNamesForProfile('collaborate')).toEqual(COLLABORATE_TOOLS);
  });

  it('returns the exact Operate tools', () => {
    expect(toolNamesForProfile('operate')).toEqual(OPERATE_TOOLS);
  });

  it('returns defensive tool arrays', () => {
    const tools = toolNamesForProfile('observe');
    tools.push('bash');

    expect(toolNamesForProfile('observe')).toEqual(OBSERVE_TOOLS);
  });

  it.each([
    ['observe', OBSERVE_TOOLS],
    ['collaborate', COLLABORATE_TOOLS],
    ['operate', OPERATE_TOOLS],
  ] as const)(
    'derives %s from the exact set regardless of order',
    (profile, tools) => {
      expect(deriveAccessProfile([...tools].reverse())).toBe(profile);
    },
  );

  it('derives custom for an unmatched set', () => {
    expect(deriveAccessProfile(['read_file'])).toBe('custom');
  });

  it('derives custom when an otherwise exact profile contains a duplicate', () => {
    expect(deriveAccessProfile([...OBSERVE_TOOLS, 'read_file'])).toBe('custom');
  });
});

describe('selectMainAgent', () => {
  it('prefers the persisted agency lead over creation-time ties or older workers', () => {
    const worker = agent('a-worker', 10);
    const lead = { ...agent('z-lead', 20), workspaceRole: 'lead' as const };
    expect(selectMainAgent([worker, lead])).toBe(lead);
  });
  it('returns null when no agents exist', () => {
    expect(selectMainAgent([])).toBeNull();
  });

  it('returns the oldest persisted agent without mutating the input', () => {
    const oldest = agent('oldest', 10);
    const newer = agent('newer', 20);
    const agents = [newer, oldest];

    expect(selectMainAgent(agents)).toBe(oldest);
    expect(agents).toEqual([newer, oldest]);
  });

  it('breaks equal creation-time ties by id', () => {
    const second = agent('b-agent', 10);
    const first = agent('a-agent', 10);

    expect(selectMainAgent([second, first])).toBe(first);
  });
});
