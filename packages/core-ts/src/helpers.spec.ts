import { describe, expect, it } from 'vitest';
import { action, agent, plugin } from './helpers.js';
import type { Action, AgentConfig, Plugin } from './types/index.js';

describe('builder helpers', () => {
  it('returns the original config objects unchanged', () => {
    const agentConfig: AgentConfig = { name: 'manager', model: 'gpt-5.4' };
    const pluginConfig: Plugin = {
      name: 'memory_search',
      description: 'search memory',
    };
    const actionConfig: Action = {
      name: 'delegate',
      description: 'delegate task',
      parametersSchema: {},
      handler: async () => ({
        status: 'success',
        data: { text: 'delegated' },
        durationMs: 0,
      }),
    };

    expect(agent(agentConfig)).toBe(agentConfig);
    expect(plugin(pluginConfig)).toBe(pluginConfig);
    expect(action(actionConfig)).toBe(actionConfig);
  });
});
