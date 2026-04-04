#!/usr/bin/env node
import { Command } from 'commander';
import { realpathSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { runCommand } from './commands/run.js';
import { chatCommand } from './commands/chat.js';
import { createCommand } from './commands/create.js';
import { launchCommand } from './commands/launch.js';
import { agentsCommand } from './commands/agents.js';
export { createCliDaemonClient } from './client.js';

export function buildProgram(): Command {
  const program = new Command();

  program
    .name('animaos')
    .description('animaOS-SWARM — Command & control your AI agent swarms')
    .version('0.0.1');

  program.addCommand(runCommand);
  program.addCommand(chatCommand);
  program.addCommand(createCommand);
  program.addCommand(launchCommand);
  program.addCommand(agentsCommand);

  return program;
}

export async function main(argv = process.argv): Promise<void> {
  await buildProgram().parseAsync(argv);
}

function normalizeExecutablePath(filePath: string): string {
  const normalizedPath = (() => {
    try {
      return realpathSync(filePath);
    } catch {
      return resolve(filePath);
    }
  })();

  return process.platform === 'win32'
    ? normalizedPath.toLowerCase()
    : normalizedPath;
}

function isDirectExecution(): boolean {
  const entrypoint = process.argv[1];

  if (!entrypoint) {
    return false;
  }

  return (
    normalizeExecutablePath(fileURLToPath(import.meta.url)) ===
    normalizeExecutablePath(entrypoint)
  );
}

if (isDirectExecution()) {
  void main();
}
