import React from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { MessageEntry, ToolEntry } from '../types.js';
import { TraceView } from './trace-view.js';
import { cleanupInk, pressInkKey, renderInk } from '../test-harness.js';

afterEach(() => {
  cleanupInk();
  vi.clearAllMocks();
});

function createMessageEntry(
  overrides: Partial<MessageEntry> = {}
): MessageEntry {
  return {
    id: 'msg-1',
    from: 'user',
    to: 'manager',
    content: 'Inspect trace',
    timestamp: 1,
    ...overrides,
  };
}

function createToolEntry(overrides: Partial<ToolEntry> = {}): ToolEntry {
  return {
    id: 'tool-1',
    agentId: 'launch:manager',
    agentName: 'manager',
    toolName: 'memory_search',
    args: { query: 'trace' },
    status: 'success',
    result: 'Trace result',
    durationMs: 4,
    timestamp: 2,
    ...overrides,
  };
}

describe('TraceView interactions', () => {
  it('navigates between tool and message entries with arrow keys', async () => {
    const onBack = vi.fn();

    const rendered = renderInk(
      <TraceView
        messages={[createMessageEntry()]}
        tools={[createToolEntry()]}
        onBack={onBack}
      />
    );

    expect(rendered.lastFrame()).toContain('Trace');
    expect(rendered.lastFrame()).toContain('Tool call');
    expect(rendered.lastFrame()).toContain('Trace result');

    await pressInkKey(rendered, '\u001B[A');

    expect(rendered.lastFrame()).toContain('Message');
    expect(rendered.lastFrame()).toContain('Inspect trace');

    await pressInkKey(rendered, '\u001B[B');

    expect(rendered.lastFrame()).toContain('Tool call');
    expect(rendered.lastFrame()).toContain('memory_search');

    await pressInkKey(rendered, 'b');

    expect(onBack).toHaveBeenCalledOnce();
  });

  it('searches trace entries with / and steps between matches with n and N', async () => {
    const onBack = vi.fn();

    const rendered = renderInk(
      <TraceView
        messages={[
          createMessageEntry({
            id: 'msg-1',
            content: 'Alpha message',
            timestamp: 1,
          }),
          createMessageEntry({
            id: 'msg-2',
            content: 'Gamma note',
            timestamp: 2,
          }),
        ]}
        tools={[
          createToolEntry({
            id: 'tool-1',
            toolName: 'alpha_lookup',
            result: 'Alpha tool result',
            timestamp: 3,
          }),
        ]}
        onBack={onBack}
      />
    );

    await pressInkKey(rendered, '/');
    await pressInkKey(rendered, 'a');
    await pressInkKey(rendered, 'l');
    await pressInkKey(rendered, 'p');
    await pressInkKey(rendered, 'h');
    await pressInkKey(rendered, 'a');
    await pressInkKey(rendered, '\r');

    expect(rendered.lastFrame()).toContain('2/2');
    expect(rendered.lastFrame()).toContain('Tool call');
    expect(rendered.lastFrame()).toContain('Alpha tool result');

    await pressInkKey(rendered, 'N');

    expect(rendered.lastFrame()).toContain('1/2');
    expect(rendered.lastFrame()).toContain('Message');
    expect(rendered.lastFrame()).toContain('Alpha message');

    await pressInkKey(rendered, 'n');

    expect(rendered.lastFrame()).toContain('2/2');
    expect(rendered.lastFrame()).toContain('Tool call');
  });

  it('shows the empty state and returns with q', async () => {
    const onBack = vi.fn();

    const rendered = renderInk(
      <TraceView messages={[]} tools={[]} onBack={onBack} />
    );

    expect(rendered.lastFrame()).toContain('No messages or tool activity yet.');

    await pressInkKey(rendered, 'q');

    expect(onBack).toHaveBeenCalledOnce();
  });
});
