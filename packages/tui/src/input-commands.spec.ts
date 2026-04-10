import { describe, expect, it } from 'vitest';
import { buildAppInputCommands } from './input-commands.js';

function commandNames(commands: Array<{ name: string }>): string[] {
  return commands.map((command) => command.name);
}

describe('input command helpers', () => {
  it('removes retry and exposes health commands when task entry is blocked', () => {
    const commands = buildAppInputCommands({
      interactive: true,
      hasSavedRuns: true,
      taskEntryBlockedByDaemon: true,
      showResultCommands: true,
    });

    expect(commandNames(commands.swarmInputCommands)).not.toContain('retry');
    expect(commandNames(commands.resultInputCommands)).toEqual([
      'back',
      'resume',
      'rename',
      'delete',
      'undo',
      'undo-drop',
      'undo-status',
      'health',
      'help',
    ]);
    expect(commandNames(commands.helpCommands)).toEqual(
      commandNames(commands.resultInputCommands)
    );
  });

  it('keeps retry available in interactive healthy sessions', () => {
    const commands = buildAppInputCommands({
      interactive: true,
      hasSavedRuns: true,
      taskEntryBlockedByDaemon: false,
      showResultCommands: true,
    });

    expect(commandNames(commands.swarmInputCommands)).toContain('retry');
    expect(commandNames(commands.resultInputCommands)).toContain('retry');
    expect(commandNames(commands.resultInputCommands)).not.toContain('health');
  });

  it('limits result commands to back when nothing can be resumed or retried', () => {
    const commands = buildAppInputCommands({
      interactive: false,
      hasSavedRuns: false,
      taskEntryBlockedByDaemon: false,
      showResultCommands: true,
    });

    expect(commandNames(commands.resultInputCommands)).toEqual(['back']);
  });
});
