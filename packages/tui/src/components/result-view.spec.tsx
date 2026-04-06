import React from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { ResultEntry } from './result-log.js';
import { ResultView } from './result-view.js';
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

describe('ResultView rendering', () => {
  it('renders a successful result with the optional hint', () => {
    const rendered = renderInk(
      <ResultView
        entry={createResultEntry({
          task: 'Ship the patch',
          result: 'Done',
          label: 'Release prep',
        })}
        onBack={() => undefined}
        hint="Resumed last run. Type /retry to run it again or /back to return."
      />
    );

    expect(rendered.lastFrame()).toContain('Result — type /back to return');
    expect(rendered.lastFrame()).toContain('Resumed last run.');
    expect(rendered.lastFrame()).toContain('Saved run:');
    expect(rendered.lastFrame()).toContain('Release prep');
    expect(rendered.lastFrame()).toContain('Task:');
    expect(rendered.lastFrame()).toContain('Ship the patch');
    expect(rendered.lastFrame()).toContain('Result:');
    expect(rendered.lastFrame()).toContain('Done');
  });

  it('renders an unlabeled saved-run note when provided', () => {
    const rendered = renderInk(
      <ResultView
        entry={createResultEntry({ task: 'Ship the patch', result: 'Done' })}
        onBack={() => undefined}
        note="Type /rename <label> to name this saved run."
      />
    );

    expect(rendered.lastFrame()).toContain(
      'Type /rename <label> to name this saved run.'
    );
    expect(rendered.lastFrame()).not.toContain('Saved run:');
  });

  it('renders an error result without a hint', () => {
    const rendered = renderInk(
      <ResultView
        entry={createResultEntry({
          task: 'Ship the patch',
          result: 'Failed to reach daemon',
          isError: true,
        })}
        onBack={() => undefined}
      />
    );

    expect(rendered.lastFrame()).toContain('Task:');
    expect(rendered.lastFrame()).toContain('Ship the patch');
    expect(rendered.lastFrame()).toContain('Error:');
    expect(rendered.lastFrame()).toContain('Failed to reach daemon');
    expect(rendered.lastFrame()).not.toContain('Resumed last run.');
  });
});
