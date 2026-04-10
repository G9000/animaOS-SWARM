import { describe, expect, it } from 'vitest';
import {
  BM25,
  MemoryManager,
  TaskHistory,
  createMemoryPlugin,
} from './index.js';

describe('memory package root exports', () => {
  it('supports a minimal consumer flow from the package entrypoint', () => {
    const index = new BM25();
    index.addDocument('doc-1', 'launch warning recovered after health check');
    index.addDocument('doc-2', 'unrelated note');

    expect(index.search('launch')[0]?.id).toBe('doc-1');

    const history = new TaskHistory();
    history.record({
      id: 'task-1',
      agentId: 'agent-1',
      task: 'Check daemon health',
      result: 'Daemon recovered after manual recheck',
      status: 'success',
      timestamp: 1,
      durationMs: 10,
      tokensUsed: 12,
    });

    expect(history.search('daemon recovered')[0]).toMatchObject({
      id: 'task-1',
      agentId: 'agent-1',
    });

    const manager = new MemoryManager();
    const memory = manager.add({
      agentId: 'agent-1',
      agentName: 'manager',
      type: 'fact',
      content: 'Launch recovered after /health.',
      importance: 0.8,
      tags: ['launch', 'health'],
    });

    expect(manager.search('health')[0]).toMatchObject({
      id: memory.id,
      agentName: 'manager',
    });

    const plugin = createMemoryPlugin(manager);
    expect(plugin.name).toBe('memory');
    expect(plugin.actions?.map((action) => action.name)).toEqual([
      'memory_search',
      'memory_recent',
    ]);
  });
});
