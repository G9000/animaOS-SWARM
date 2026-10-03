import { describe, expect, it, vi } from 'vitest';

import {
  MAX_SKILL_COMMAND_DESCRIPTION,
  SLASH_COMMANDS,
  parseSlashCommand,
  runSlashCommand,
  skillSlashCommands,
  slashSuggestions,
} from './slash-commands';
import { skillFixture } from '../test/skills';

describe('parseSlashCommand', () => {
  it('reads a known command and the text after it', () => {
    expect(parseSlashCommand('  /rename   Weekend trip ')).toEqual({
      command: expect.objectContaining({ name: 'rename' }),
      argument: 'Weekend trip',
    });
    expect(parseSlashCommand('/new')).toEqual({
      command: expect.objectContaining({ name: 'new' }),
      argument: '',
    });
  });

  it('leaves anything else to be sent as a message', () => {
    expect(parseSlashCommand('/unknown thing')).toBeNull();
    expect(parseSlashCommand('hello /new')).toBeNull();
    expect(parseSlashCommand('/New')).toBeNull();
    expect(parseSlashCommand('/')).toBeNull();
  });
});

describe('slashSuggestions', () => {
  it('suggests commands while only the first word is typed', () => {
    expect(slashSuggestions('/').map((command) => command.name)).toEqual(
      SLASH_COMMANDS.map((command) => command.name),
    );
    expect(slashSuggestions('/co').map((command) => command.name)).toEqual([
      'compact',
    ]);
    expect(slashSuggestions('/rename x')).toEqual([]);
    expect(slashSuggestions('hi')).toEqual([]);
  });
});

describe('runSlashCommand', () => {
  it('runs a command where it is available and says why otherwise', () => {
    const rename = vi.fn();
    const renameCommand = parseSlashCommand('/rename Offsite')!;

    expect(runSlashCommand(renameCommand, { rename })).toBeNull();
    expect(rename).toHaveBeenCalledWith('Offsite');
    expect(runSlashCommand(parseSlashCommand('/rename')!, { rename })).toBe(
      'Add a title after /rename.',
    );
    expect(runSlashCommand(parseSlashCommand('/stop')!, { rename })).toBe(
      '/stop is not available here.',
    );
  });

  it('refuses extra text after a command that takes none, discarding nothing (S3b-D)', () => {
    const newChat = vi.fn();
    const help = vi.fn();
    expect(
      runSlashCommand(parseSlashCommand('/new write a poem')!, {
        new: newChat,
      }),
    ).toBe('/new takes no text.');
    expect(newChat).not.toHaveBeenCalled();
    expect(runSlashCommand(parseSlashCommand('/help me')!, { help })).toBe(
      '/help takes no text.',
    );
    expect(help).not.toHaveBeenCalled();
    // Still runs with no text at all.
    expect(
      runSlashCommand(parseSlashCommand('/new')!, { new: newChat }),
    ).toBeNull();
    expect(newChat).toHaveBeenCalledWith('');
  });
});

describe('skill commands', () => {
  const skills = [
    skillFixture('notes', { description: 'Take notes' }),
    skillFixture('weekly-2', { description: 'd'.repeat(120) }),
    skillFixture('off', { enabled: false }),
    skillFixture('changed', { status: 'changed' }),
    skillFixture('help'),
  ];

  it('offers enabled active skills that do not shadow a built-in command', () => {
    const commands = skillSlashCommands(skills);
    expect(commands.map((command) => command.name)).toEqual([
      'notes',
      'weekly-2',
    ]);
    expect(commands[0]).toEqual({
      name: 'notes',
      description: 'Take notes',
      placeholder: '<request>',
      skill: 'notes',
    });
    expect(commands[1].description).toHaveLength(MAX_SKILL_COMMAND_DESCRIPTION);
  });

  it('parses slugs with digits and hyphens and never runs a skill as a command', () => {
    const all = [...SLASH_COMMANDS, ...skillSlashCommands(skills)];
    expect(parseSlashCommand('/weekly-2 plan it', all)).toEqual({
      command: expect.objectContaining({ skill: 'weekly-2' }),
      argument: 'plan it',
    });
    expect(
      slashSuggestions('/wee', all).map((command) => command.name),
    ).toEqual(['weekly-2']);
    expect(parseSlashCommand('/weekly-2', SLASH_COMMANDS)).toBeNull();
    const parsed = parseSlashCommand('/notes', all)!;
    expect(runSlashCommand(parsed, {})).toBe('/notes is not available here.');
  });
});
