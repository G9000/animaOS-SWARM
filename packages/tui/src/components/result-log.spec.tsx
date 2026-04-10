import React from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { ResultEntry } from './result-log.js';
import { ResultLog } from './result-log.js';
import { cleanupInk, renderInk } from '../test-harness.js';

afterEach(() => {
  cleanupInk();
  vi.clearAllMocks();
});

function createResultEntry(overrides: Partial<ResultEntry> = {}): ResultEntry {
  return {
    id: 'run-1',
    timestamp: 1,
    task: 'Earlier task',
    result: 'Earlier result',
    isError: false,
    ...overrides,
  };
}

describe('ResultLog rendering', () => {
  it('renders only the three most recent runs', () => {
    const rendered = renderInk(
      <ResultLog
        results={[
          createResultEntry({ id: 'run-1', task: 'First task' }),
          createResultEntry({ id: 'run-2', timestamp: 2, task: 'Second task' }),
          createResultEntry({ id: 'run-3', timestamp: 3, task: 'Third task' }),
          createResultEntry({ id: 'run-4', timestamp: 4, task: 'Fourth task' }),
        ]}
      />
    );

    expect(rendered.lastFrame()).toContain('Past runs');
    expect(rendered.lastFrame()).not.toContain('First task');
    expect(rendered.lastFrame()).toContain('Second task');
    expect(rendered.lastFrame()).toContain('Third task');
    expect(rendered.lastFrame()).toContain('Fourth task');
  });

  it('truncates long task and result text and preserves error styling markers', () => {
    const longTask = 'T'.repeat(60);
    const longResult = 'R'.repeat(210);

    const rendered = renderInk(
      <ResultLog
        results={[
          createResultEntry({
            id: 'run-1',
            task: longTask,
            result: longResult,
            isError: true,
          }),
        ]}
      />
    );

    expect(rendered.lastFrame()).toContain(`✗ ${'T'.repeat(52)}...`);
    expect(rendered.lastFrame()).not.toContain(longResult);
    expect(rendered.lastFrame()).toMatch(/R{3,}\.\.\./);
    expect(rendered.lastFrame()).toContain(
      '/history browse all /retry rerun last'
    );
  });

  it('renders a saved label separately from the task summary when present', () => {
    const rendered = renderInk(
      <ResultLog
        results={[
          createResultEntry({
            id: 'run-1',
            label: 'Release prep',
            task: 'Ship the patch and update docs',
            result: 'Done',
          }),
        ]}
      />
    );

    expect(rendered.lastFrame()).toContain('✓ Release prep');
    expect(rendered.lastFrame()).toContain(
      'task: Ship the patch and update docs'
    );
  });

  it('can hide the retry hint from the footer', () => {
    const rendered = renderInk(
      <ResultLog results={[createResultEntry()]} showRetryHint={false} />
    );

    expect(rendered.lastFrame()).toContain('/history browse all');
    expect(rendered.lastFrame()).not.toContain('/retry rerun last');
  });
});
