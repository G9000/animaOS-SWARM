import type { AgentDetail } from './types';

export type AccessProfile = 'observe' | 'collaborate' | 'operate';
export type DerivedAccessProfile = AccessProfile | 'custom';

const COMMON_TOOLS = [
  'memory_search',
  'memory_add',
  'recent_memories',
  'get_current_time',
  'calculate',
] as const;

const OBSERVE_TOOLS = [
  ...COMMON_TOOLS,
  'read_file',
  'list_dir',
  'glob',
  'grep',
  'todo_read',
] as const;

const COLLABORATE_TOOLS = [
  ...OBSERVE_TOOLS,
  'write_file',
  'edit_file',
  'multi_edit',
  'todo_write',
] as const;

const OPERATE_TOOLS = [
  ...COLLABORATE_TOOLS,
  'bash',
  'bg_start',
  'bg_output',
  'bg_stop',
  'bg_list',
] as const;

export const ACCESS_PROFILES = {
  observe: {
    label: 'Observe',
    summary: 'Inspect workspace files and todos.',
    risk: 'Read-only workspace access; cannot modify files or execute processes.',
    tools: OBSERVE_TOOLS,
  },
  collaborate: {
    label: 'Collaborate',
    summary: 'Inspect and update workspace files and todos.',
    risk: 'Can modify workspace files; cannot execute processes.',
    tools: COLLABORATE_TOOLS,
  },
  operate: {
    label: 'Operate',
    summary: 'Inspect, update, and run work in the workspace.',
    risk: 'Can execute shell commands and manage background processes.',
    tools: OPERATE_TOOLS,
  },
} as const;

const PROFILE_NAMES: readonly AccessProfile[] = [
  'observe',
  'collaborate',
  'operate',
];

export function toolNamesForProfile(profile: AccessProfile): string[] {
  return [...ACCESS_PROFILES[profile].tools];
}

export function deriveAccessProfile(
  toolNames: readonly string[],
): DerivedAccessProfile {
  const uniqueTools = new Set(toolNames);
  if (uniqueTools.size !== toolNames.length) {
    return 'custom';
  }

  for (const profile of PROFILE_NAMES) {
    const profileTools = ACCESS_PROFILES[profile].tools;
    if (
      profileTools.length === uniqueTools.size &&
      profileTools.every((tool) => uniqueTools.has(tool))
    ) {
      return profile;
    }
  }

  return 'custom';
}

export function selectMainAgent(
  agents: readonly AgentDetail[],
): AgentDetail | null {
  if (agents.length === 0) {
    return null;
  }

  return [...agents].sort((left, right) => {
    const leadOrder =
      Number(right.workspaceRole === 'lead') -
      Number(left.workspaceRole === 'lead');
    if (leadOrder !== 0) return leadOrder;
    const creationOrder = left.created_at_ms - right.created_at_ms;
    if (creationOrder !== 0) {
      return creationOrder;
    }

    if (left.id < right.id) {
      return -1;
    }
    if (left.id > right.id) {
      return 1;
    }
    return 0;
  })[0];
}
