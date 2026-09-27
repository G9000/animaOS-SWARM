import { describe, expect, it, vi } from 'vitest';

import {
  SLASH_COMMANDS,
  parseSlashCommand,
  runSlashCommand,
  slashSuggestions,
} from './slash-commands';

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
