import { describe, it, expect } from 'vitest';
import { TaskHistory } from './task-history.js';

function makeEntry(
  id: string,
  task: string,
  result: string,
  agentId = 'agent-1'
) {
  return {
    id,
    agentId,
    task,
    result,
    status: 'success' as const,
    timestamp: Date.now(),
    durationMs: 100,
    tokensUsed: 50,
  };
}

describe('TaskHistory', () => {
  it('should record and search entries', () => {
    const history = new TaskHistory();
    history.record(
      makeEntry(
        '1',
        'Research AI agents',
        'Found 10 papers on multi-agent systems'
      )
    );
    history.record(
      makeEntry('2', 'Write code for API', 'Created REST endpoints')
    );
    history.record(
      makeEntry('3', 'Deploy to production', 'Deployed successfully')
    );

    const results = history.search('agent');
    expect(results.length).toBeGreaterThan(0);
    expect(results[0].task).toContain('AI agents');
  });

  it('should get recent entries', () => {
    const history = new TaskHistory();
    history.record({ ...makeEntry('1', 'task 1', 'r1'), timestamp: 100 });
    history.record({ ...makeEntry('2', 'task 2', 'r2'), timestamp: 200 });
    history.record({ ...makeEntry('3', 'task 3', 'r3'), timestamp: 300 });

    const recent = history.getRecent(2);
    expect(recent).toHaveLength(2);
    expect(recent[0].id).toBe('3');
    expect(recent[1].id).toBe('2');
  });

  it('should filter by agent', () => {
    const history = new TaskHistory();
    history.record(makeEntry('1', 'task a', 'result a', 'agent-1'));
    history.record(makeEntry('2', 'task b', 'result b', 'agent-2'));
    history.record(makeEntry('3', 'task c', 'result c', 'agent-1'));

    const agentTasks = history.getByAgent('agent-1');
    expect(agentTasks).toHaveLength(2);
  });

  it('should clear all entries', () => {
    const history = new TaskHistory();
    history.record(makeEntry('1', 'task', 'result'));
    history.clear();

    expect(history.size).toBe(0);
    expect(history.search('task')).toHaveLength(0);
  });
});
