import { describe, expect, it } from 'vitest';
import { buildProgram } from './index.js';

describe('cli package root exports', () => {
  it('builds the expected top-level command surface', () => {
    const program = buildProgram();

    expect(program.name()).toBe('animaos');
    expect(program.description()).toContain(
      'Command & control your AI agent swarms'
    );
    expect(program.commands.map((command) => command.name())).toEqual([
      'run',
      'chat',
      'create',
      'launch',
      'agents',
      'mod',
    ]);
  });
});
