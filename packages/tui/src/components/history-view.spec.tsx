import React from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { ResultEntry } from './result-log.js';
import { HistoryView } from './history-view.js';
import { cleanupInk, pressInkKey, renderInk } from '../test-harness.js';

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

describe('HistoryView interactions', () => {
  it('navigates entries with arrow keys and retries the selected run', async () => {
    const onBack = vi.fn();
    const onRetry = vi.fn();

    const first = createResultEntry({
      id: 'run-1',
      task: 'First task',
      result: 'First result',
    });
    const second = createResultEntry({
      id: 'run-2',
      timestamp: 2,
      task: 'Second task',
      result: 'Second result',
    });

    const rendered = renderInk(
      <HistoryView
        results={[first, second]}
        onBack={onBack}
        onRetry={onRetry}
      />
    );

    expect(rendered.lastFrame()).toContain('History');
    expect(rendered.lastFrame()).toContain('Second task');
    expect(rendered.lastFrame()).toContain('Second result');

    await pressInkKey(rendered, '\u001B[A');

    expect(rendered.lastFrame()).toContain('First task');
    expect(rendered.lastFrame()).toContain('First result');

    await pressInkKey(rendered, 'r');

    expect(onRetry).toHaveBeenCalledWith(
      expect.objectContaining({
        id: 'run-1',
        task: 'First task',
        result: 'First result',
      })
    );

    await pressInkKey(rendered, 'q');

    expect(onBack).toHaveBeenCalledOnce();
  });

  it('searches history entries with / and steps between matches with n and N', async () => {
    const onBack = vi.fn();

    const rendered = renderInk(
      <HistoryView
        results={[
          createResultEntry({
            id: 'run-1',
            task: 'Alpha fix',
            label: 'Hotfix Alpha',
            result: 'First alpha result',
          }),
          createResultEntry({
            id: 'run-2',
            timestamp: 2,
            task: 'Beta patch',
            result: 'Second result',
          }),
          createResultEntry({
            id: 'run-3',
            timestamp: 3,
            task: 'Alpha docs',
            result: 'Third alpha result',
          }),
        ]}
        onBack={onBack}
      />
    );

    await pressInkKey(rendered, '/');
    await pressInkKey(rendered, 'h');
    await pressInkKey(rendered, 'o');
    await pressInkKey(rendered, 't');
    await pressInkKey(rendered, 'f');
    await pressInkKey(rendered, 'i');
    await pressInkKey(rendered, 'x');
    await pressInkKey(rendered, '\r');

    expect(rendered.lastFrame()).toContain('1/1');
    expect(rendered.lastFrame()).toContain('Hotfix Alpha');
    expect(rendered.lastFrame()).toContain('Alpha fix');
    expect(rendered.lastFrame()).toContain('First alpha result');
  });

  it('shows the empty state and returns with escape', async () => {
    const onBack = vi.fn();

    const rendered = renderInk(<HistoryView results={[]} onBack={onBack} />);

    expect(rendered.lastFrame()).toContain('No runs recorded yet.');

    await pressInkKey(rendered, '\u001B');

    expect(onBack).toHaveBeenCalledOnce();
  });

  it('shows undo affordances in the empty state when deleted runs are queued', async () => {
    const onBack = vi.fn();
    const onUndoDelete = vi.fn();
    const onDropOldestUndo = vi.fn();

    const rendered = renderInk(
      <HistoryView
        results={[]}
        onBack={onBack}
        onUndoDelete={onUndoDelete}
        onDropOldestUndo={onDropOldestUndo}
        undoDeleteLabel="release prep"
        dropOldestUndoLabel="hotfix prep"
        undoDeleteCount={2}
        title="Resume"
      />
    );

    expect(rendered.lastFrame()).toContain('No runs recorded yet.');
    expect(rendered.lastFrame()).toContain('u undo delete');
    expect(rendered.lastFrame()).toContain('D drop oldest undo');
    expect(rendered.lastFrame()).toContain('Press u to restore release prep.');
    expect(rendered.lastFrame()).toContain('1 more deleted saved run queued.');
    expect(rendered.lastFrame()).toContain(
      'Press D to discard oldest queued undo: hotfix prep.'
    );

    await pressInkKey(rendered, 'u');

    expect(onUndoDelete).toHaveBeenCalledOnce();

    await pressInkKey(rendered, 'D');

    expect(onDropOldestUndo).not.toHaveBeenCalled();
    expect(rendered.lastFrame()).toContain('D confirm drop oldest undo');
    expect(rendered.lastFrame()).toContain('Confirm oldest undo discard');
    expect(rendered.lastFrame()).toContain(
      'Press D again to discard oldest queued undo: hotfix prep.'
    );

    await pressInkKey(rendered, 'D');

    expect(onDropOldestUndo).toHaveBeenCalledOnce();
  });

  it('selects the highlighted run with enter when a picker action is provided', async () => {
    const onBack = vi.fn();
    const onSelect = vi.fn();

    const first = createResultEntry({
      id: 'run-1',
      task: 'First task',
      result: 'First result',
    });
    const second = createResultEntry({
      id: 'run-2',
      timestamp: 2,
      task: 'Second task',
      result: 'Second result',
    });

    const rendered = renderInk(
      <HistoryView
        results={[first, second]}
        onBack={onBack}
        onSelect={onSelect}
        title="Resume"
        selectActionLabel="resume"
      />
    );

    expect(rendered.lastFrame()).toContain('Resume');
    expect(rendered.lastFrame()).toContain('enter resume');

    await pressInkKey(rendered, '\u001B[A');
    await pressInkKey(rendered, '\r');

    expect(onSelect).toHaveBeenCalledWith(
      expect.objectContaining({
        id: 'run-1',
        task: 'First task',
      })
    );
  });

  it('deletes the highlighted run with x when a delete action is provided', async () => {
    const onBack = vi.fn();
    const onDelete = vi.fn();

    const first = createResultEntry({
      id: 'run-1',
      task: 'First task',
      result: 'First result',
      label: 'release prep',
    });
    const second = createResultEntry({
      id: 'run-2',
      timestamp: 2,
      task: 'Second task',
      result: 'Second result',
    });

    const rendered = renderInk(
      <HistoryView
        results={[first, second]}
        onBack={onBack}
        onDelete={onDelete}
        title="Resume"
        selectActionLabel="resume"
        initialSelection="first"
      />
    );

    expect(rendered.lastFrame()).toContain('x delete');
    expect(rendered.lastFrame()).toContain('release prep');

    await pressInkKey(rendered, 'x');

    expect(onDelete).not.toHaveBeenCalled();
    expect(rendered.lastFrame()).toContain('x confirm delete');
    expect(rendered.lastFrame()).toContain('Confirm delete');
    expect(rendered.lastFrame()).toContain(
      'Press x again to delete this saved run.'
    );

    await pressInkKey(rendered, 'x');

    expect(onDelete).toHaveBeenCalledWith(
      expect.objectContaining({
        id: 'run-1',
        label: 'release prep',
      })
    );
  });

  it('restores the last deleted run with u when undo is available', async () => {
    const onBack = vi.fn();
    const onUndoDelete = vi.fn();

    const rendered = renderInk(
      <HistoryView
        results={[
          createResultEntry({
            id: 'run-2',
            timestamp: 2,
            task: 'Second task',
            result: 'Second result',
          }),
        ]}
        onBack={onBack}
        onUndoDelete={onUndoDelete}
        undoDeleteLabel="release prep"
        undoDeleteCount={2}
        title="Resume"
        selectActionLabel="resume"
        initialSelection="first"
      />
    );

    expect(rendered.lastFrame()).toContain('u undo delete');
    expect(rendered.lastFrame()).toContain('Undo delete');
    expect(rendered.lastFrame()).toContain('Press u to restore release prep.');
    expect(rendered.lastFrame()).toContain('1 more deleted saved run queued.');

    await pressInkKey(rendered, 'u');

    expect(onUndoDelete).toHaveBeenCalledOnce();
  });
});
