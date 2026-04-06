import React from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { AgentProfile } from '../types.js';
import { AgentsPanel } from './agents-panel.js';
import { cleanupInk, pressInkKey, renderInk } from '../test-harness.js';

afterEach(() => {
  cleanupInk();
  vi.clearAllMocks();
});

function createAgentProfile(
  overrides: Partial<AgentProfile> = {}
): AgentProfile {
  return {
    name: 'manager',
    role: 'orchestrator',
    bio: 'Keeps the swarm aligned.',
    lore: 'Was built to coordinate specialized agents.',
    adjectives: ['sharp', 'methodical'],
    topics: ['coordination', 'trace'],
    knowledge: ['workflow state', 'handoff rules'],
    style: 'Direct and structured.',
    system: 'Keep the swarm aligned and moving.',
    ...overrides,
  };
}

describe('AgentsPanel interactions', () => {
  it('navigates from the list into detail view and back', async () => {
    const onBack = vi.fn();
    const onSave = vi.fn();

    const rendered = renderInk(
      <AgentsPanel
        profiles={[
          createAgentProfile(),
          createAgentProfile({
            name: 'writer',
            role: 'worker',
            bio: 'Turns research into concise output.',
            adjectives: ['concise', 'fast'],
          }),
        ]}
        onBack={onBack}
        onSave={onSave}
      />
    );

    expect(rendered.lastFrame()).toContain('Agents (2)');
    expect(rendered.lastFrame()).toContain('manager');

    await pressInkKey(rendered, '\u001B[B');
    await pressInkKey(rendered, '\r');

    expect(rendered.lastFrame()).toContain('writer');
    expect(rendered.lastFrame()).toContain(
      'Turns research into concise output.'
    );
    expect(rendered.lastFrame()).toContain('q back to list');

    await pressInkKey(rendered, 'q');

    expect(rendered.lastFrame()).toContain('Agents (2)');
    expect(onBack).not.toHaveBeenCalled();
  });

  it('searches agent profiles from the list and steps between matches', async () => {
    const onBack = vi.fn();
    const onSave = vi.fn();

    const rendered = renderInk(
      <AgentsPanel
        profiles={[
          createAgentProfile(),
          createAgentProfile({
            name: 'writer',
            role: 'worker',
            bio: 'Turns research into output.',
            adjectives: ['concise', 'fast'],
            topics: ['docs'],
          }),
          createAgentProfile({
            name: 'reviewer',
            role: 'worker',
            bio: 'Reviews output for correctness.',
            adjectives: ['sharp', 'thorough'],
            topics: ['qa'],
          }),
        ]}
        onBack={onBack}
        onSave={onSave}
      />
    );

    await pressInkKey(rendered, '/');
    for (const char of 'output') {
      await pressInkKey(rendered, char);
    }
    await pressInkKey(rendered, '\r');

    expect(rendered.lastFrame()).toContain('1/2');

    await pressInkKey(rendered, '\r');

    expect(rendered.lastFrame()).toContain('writer');
    expect(rendered.lastFrame()).toContain('Turns research into output.');

    await pressInkKey(rendered, 'q');
    await pressInkKey(rendered, 'n');

    expect(rendered.lastFrame()).toContain('2/2');

    await pressInkKey(rendered, '\r');

    expect(rendered.lastFrame()).toContain('reviewer');
    expect(rendered.lastFrame()).toContain('Reviews output for correctness.');

    await pressInkKey(rendered, 'q');
    await pressInkKey(rendered, 'N');

    expect(rendered.lastFrame()).toContain('1/2');
  });

  it('edits the selected agent bio and saves with ctrl+s', async () => {
    const onBack = vi.fn();
    const onSave = vi.fn();

    const rendered = renderInk(
      <AgentsPanel
        profiles={[createAgentProfile()]}
        onBack={onBack}
        onSave={onSave}
      />
    );

    await pressInkKey(rendered, 'e');

    expect(rendered.lastFrame()).toContain('Editing');
    expect(rendered.lastFrame()).toContain('bio');

    for (const char of ' Updated') {
      await pressInkKey(rendered, char);
    }

    await pressInkKey(rendered, '\u0013');

    expect(onSave).toHaveBeenCalledWith(
      expect.objectContaining({
        name: 'manager',
        bio: 'Keeps the swarm aligned. Updated',
      })
    );
    expect(rendered.lastFrame()).toContain('saved to anima.yaml');
  });
});
