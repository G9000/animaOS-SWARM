import React from 'react';
import { afterEach, describe, expect, it } from 'vitest';
import { EventBus } from '@animaOS-SWARM/core';
import { App } from './index.js';
import { cleanupInk, flushInk, renderInk } from './test-harness.js';

afterEach(() => {
  cleanupInk();
});

describe('tui package root exports', () => {
  it('renders the exported App against a real event bus consumer flow', async () => {
    const eventBus = new EventBus();
    const rendered = renderInk(
      <App
        eventBus={eventBus}
        strategy="supervisor"
        task="Boundary validation"
      />
    );

    await flushInk();
    expect(rendered.lastFrame()).toContain('Boundary validation');
    expect(rendered.lastFrame()).toContain('Waiting for agents to spawn');

    await eventBus.emit(
      'agent:spawned',
      { agentId: 'launch:manager', name: 'manager' },
      'launch:manager'
    );
    await eventBus.emit(
      'task:started',
      { agentId: 'launch:manager' },
      'launch:manager'
    );
    await eventBus.emit(
      'agent:message',
      { from: 'manager', to: 'user', message: { text: 'Working on parity' } },
      'launch:manager'
    );
    await eventBus.emit(
      'tool:before',
      {
        agentId: 'launch:manager',
        toolName: 'memory_search',
        args: { query: 'parity' },
      },
      'launch:manager'
    );
    await eventBus.emit(
      'tool:after',
      {
        agentId: 'launch:manager',
        toolName: 'memory_search',
        status: 'success',
        durationMs: 12,
        result: 'done',
      },
      'launch:manager'
    );
    await eventBus.emit(
      'agent:tokens',
      { agentId: 'launch:manager', usage: { totalTokens: 42 } },
      'launch:manager'
    );
    await eventBus.emit(
      'task:completed',
      {
        agentId: 'launch:manager',
        result: { data: { text: 'Integration complete' } },
      },
      'launch:manager'
    );
    await eventBus.emit(
      'swarm:completed',
      {
        result: {
          status: 'success',
          data: { text: 'Integration complete' },
        },
      },
      'launch:manager'
    );

    await flushInk();

    expect(rendered.lastFrame()).toContain('[manager]');
    expect(rendered.lastFrame()).toContain('Working on parity');
    expect(rendered.lastFrame()).toContain('memory_search');
    expect(rendered.lastFrame()).toContain('tokens: 42');
    expect(rendered.lastFrame()).toContain('Result: Integration complete');
  });
});
