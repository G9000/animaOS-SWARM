import { describe, expect, it, vi } from 'vitest';
import type { GenerateResult, IModelAdapter } from '@animaOS-SWARM/core';
import { swarm } from './index.js';

function textAdapter(response: string): IModelAdapter {
  return {
    provider: 'test',
    generate: vi.fn().mockResolvedValue({
      content: { text: response },
      toolCalls: undefined,
      usage: { promptTokens: 1, completionTokens: 1, totalTokens: 2 },
      stopReason: 'end',
    } satisfies GenerateResult),
  };
}

describe('swarm package root exports', () => {
  it('runs a minimal swarm through the package entrypoint helper', async () => {
    const coordinator = swarm(
      {
        strategy: 'round-robin',
        manager: { name: 'manager', model: 'test-model' },
        workers: [{ name: 'worker', model: 'test-model' }],
        maxTurns: 2,
      },
      textAdapter('done')
    );

    const result = await coordinator.run('Boundary validation');

    expect(result.status).toBe('success');
    expect(coordinator.getState()).toMatchObject({
      status: 'idle',
    });
    expect(coordinator.getState().results).toHaveLength(1);
  });
});
