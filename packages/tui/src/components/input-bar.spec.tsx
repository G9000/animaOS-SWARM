import React from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  InputBar,
  type InputSuggestion,
  type SlashCommand,
} from './input-bar.js';
import {
  cleanupInk,
  pressInkKey,
  renderInk,
  submitInk,
} from '../test-harness.js';

afterEach(() => {
  cleanupInk();
  vi.clearAllMocks();
});

describe('InputBar interactions', () => {
  it('submits a plain task and resets to the placeholder', async () => {
    const onSubmit = vi.fn();

    const rendered = renderInk(<InputBar onSubmit={onSubmit} />);

    await submitInk(rendered, 'Ship the patch');

    expect(onSubmit).toHaveBeenCalledWith('Ship the patch');
    expect(rendered.lastFrame()).toContain(
      'type your task... or /help for commands'
    );
  });

  it('navigates slash command matches with arrows and submits the selected command', async () => {
    const onSubmit = vi.fn();
    const commands: SlashCommand[] = [
      { name: 'help', description: 'show available commands' },
      { name: 'history', description: 'browse past runs' },
    ];

    const rendered = renderInk(
      <InputBar onSubmit={onSubmit} commands={commands} />
    );

    await pressInkKey(rendered, '/');
    await pressInkKey(rendered, 'h');

    expect(rendered.lastFrame()).toContain('/help');
    expect(rendered.lastFrame()).toContain('/history');

    await pressInkKey(rendered, '\u001B[B');
    await pressInkKey(rendered, '\r');

    expect(onSubmit).toHaveBeenCalledWith('/history');
    expect(rendered.lastFrame()).toContain(
      'type your task... or /help for commands'
    );
  });

  it('tab-completes a command with args and submits the completed input', async () => {
    const onSubmit = vi.fn();
    const commands: SlashCommand[] = [
      { name: 'open', description: 'open a saved session', args: '<name>' },
    ];

    const rendered = renderInk(
      <InputBar onSubmit={onSubmit} commands={commands} />
    );

    await pressInkKey(rendered, '/');
    await pressInkKey(rendered, 'o');
    await pressInkKey(rendered, '\t');

    expect(rendered.lastFrame()).toContain('/open');
    expect(onSubmit).not.toHaveBeenCalled();

    for (const char of 'session-1') {
      await pressInkKey(rendered, char);
    }

    await pressInkKey(rendered, '\r');

    expect(onSubmit).toHaveBeenCalledWith('/open session-1');
  });

  it('shows input suggestions for slash command arguments and submits the completed value', async () => {
    const onSubmit = vi.fn();
    const commands: SlashCommand[] = [
      { name: 'resume', description: 'resume a saved run', args: '<label>' },
    ];
    const suggestions = (value: string): InputSuggestion[] =>
      value.startsWith('/resume ')
        ? [
            {
              label: 'launch docs',
              value: '/resume launch docs',
              description: 'Second task',
            },
            {
              label: 'launch hotfix',
              value: '/resume launch hotfix',
              description: 'First task',
            },
          ]
        : [];

    const rendered = renderInk(
      <InputBar
        onSubmit={onSubmit}
        commands={commands}
        suggestions={suggestions}
      />
    );

    for (const char of '/resume ') {
      await pressInkKey(rendered, char);
    }

    expect(rendered.lastFrame()).toContain('launch docs');
    expect(rendered.lastFrame()).toContain('launch hotfix');
    expect(rendered.lastFrame()).toContain('Second task');

    await pressInkKey(rendered, '\u001B[B');
    await pressInkKey(rendered, '\t');

    expect(onSubmit).not.toHaveBeenCalled();
    expect(rendered.lastFrame()).toContain('/resume launch hotfix');

    await pressInkKey(rendered, '\r');

    expect(onSubmit).toHaveBeenCalledWith('/resume launch hotfix');
  });

  it('recalls previous prompts with arrow keys and restores the current draft', async () => {
    const onSubmit = vi.fn();

    const rendered = renderInk(
      <InputBar onSubmit={onSubmit} history={['First task', 'Second task']} />
    );

    expect(rendered.lastFrame()).toContain('↑↓ recall previous prompts');

    for (const char of 'Current draft') {
      await pressInkKey(rendered, char);
    }

    await pressInkKey(rendered, '\u001B[A');

    expect(rendered.lastFrame()).toContain('Second task');

    await pressInkKey(rendered, '\u001B[A');

    expect(rendered.lastFrame()).toContain('First task');

    await pressInkKey(rendered, '\u001B[B');

    expect(rendered.lastFrame()).toContain('Second task');

    await pressInkKey(rendered, '\u001B[B');

    expect(rendered.lastFrame()).toContain('Current draft');
  });

  it('searches prompt history with ctrl+r, cycles matches, and accepts the selected prompt', async () => {
    const onSubmit = vi.fn();

    const rendered = renderInk(
      <InputBar
        onSubmit={onSubmit}
        history={['Alpha task', 'Beta task', 'Alpha docs']}
      />
    );

    await pressInkKey(rendered, '\u0012');

    expect(rendered.lastFrame()).toContain('ctrl+r history search');
    expect(rendered.lastFrame()).toContain('Alpha docs');

    for (const char of 'alpha') {
      await pressInkKey(rendered, char);
    }

    expect(rendered.lastFrame()).toContain('1/2');
    expect(rendered.lastFrame()).toContain('Alpha docs');

    await pressInkKey(rendered, '\u0012');

    expect(rendered.lastFrame()).toContain('2/2');
    expect(rendered.lastFrame()).toContain('Alpha task');

    await pressInkKey(rendered, '\r');

    expect(onSubmit).not.toHaveBeenCalled();
    expect(rendered.lastFrame()).toContain('Alpha task');

    await pressInkKey(rendered, '\r');

    expect(onSubmit).toHaveBeenCalledWith('Alpha task');
  });

  it('keeps ctrl+r search active when the matched history entry is a slash command', async () => {
    const onSubmit = vi.fn();
    const commands: SlashCommand[] = [
      { name: 'help', description: 'show available commands' },
    ];

    const rendered = renderInk(
      <InputBar
        onSubmit={onSubmit}
        commands={commands}
        history={['Ship patch', '/help']}
      />
    );

    await pressInkKey(rendered, '\u0012');
    await pressInkKey(rendered, '/');

    expect(rendered.lastFrame()).toContain('ctrl+r history search');
    expect(rendered.lastFrame()).toContain('/help');
    expect(onSubmit).not.toHaveBeenCalled();

    await pressInkKey(rendered, '\r');

    expect(rendered.lastFrame()).toContain('/help');
    expect(onSubmit).not.toHaveBeenCalled();

    await pressInkKey(rendered, '\r');

    expect(onSubmit).toHaveBeenCalledWith('/help');
  });

  it('ignores input while disabled', async () => {
    const onSubmit = vi.fn();

    const rendered = renderInk(<InputBar onSubmit={onSubmit} disabled />);

    expect(rendered.lastFrame()).toContain('running swarm...');

    await pressInkKey(rendered, 'x');
    await pressInkKey(rendered, '\r');

    expect(onSubmit).not.toHaveBeenCalled();
    expect(rendered.lastFrame()).toContain('running swarm...');
  });

  it('shows a command-only prompt state while still allowing slash commands', async () => {
    const onSubmit = vi.fn();

    const rendered = renderInk(
      <InputBar
        onSubmit={onSubmit}
        commandOnly
        commandOnlyHint="daemon down - tasks paused; use /health or /help"
        commandOnlyHelperText="commands only while daemon is down"
        history={['Earlier task']}
        commands={[{ name: 'health', description: 'check daemon health' }]}
      />
    );

    expect(rendered.lastFrame()).toContain(
      'daemon down - tasks paused; use /health or /help'
    );
    expect(rendered.lastFrame()).toContain(
      'commands only while daemon is down'
    );
    expect(rendered.lastFrame()).not.toContain(
      '↑↓ recall previous prompts · ctrl+r search history'
    );

    await submitInk(rendered, '/health');

    expect(onSubmit).toHaveBeenCalledWith('/health');
  });
});
