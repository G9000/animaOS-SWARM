import { join } from 'node:path';
import { Command } from 'commander';
import { readModConfig, writeModConfig } from '@animaOS-SWARM/mod-sdk';

function configPath(): string {
  return join(
    process.env['ANIMAOS_WORKSPACE_ROOT'] ?? process.cwd(),
    '.animaos',
    'mods.json'
  );
}

export function executeModEnableCommand(name: string, path = configPath()): void {
  const config = readModConfig(path);
  if (!config.enabled.includes(name)) {
    config.enabled.push(name);
    writeModConfig(path, config);
    console.log(`✓ Enabled mod: ${name}`);
  } else {
    console.log(`Mod "${name}" is already enabled`);
  }
}

export function executeModDisableCommand(name: string, path = configPath()): void {
  const config = readModConfig(path);
  const index = config.enabled.indexOf(name);
  if (index !== -1) {
    config.enabled.splice(index, 1);
    writeModConfig(path, config);
    console.log(`✓ Disabled mod: ${name}`);
  } else {
    console.log(`Mod "${name}" is not enabled`);
  }
}

export function executeModListCommand(path = configPath()): void {
  const config = readModConfig(path);
  if (config.enabled.length === 0) {
    console.log('No mods enabled');
  } else {
    console.log('Enabled mods:');
    for (const name of config.enabled) {
      console.log(`  • ${name}`);
    }
  }
}

const enableCommand = new Command('enable')
  .description('Enable a mod')
  .argument('<name>', 'Mod name')
  .action((name: string) => executeModEnableCommand(name));

const disableCommand = new Command('disable')
  .description('Disable a mod')
  .argument('<name>', 'Mod name')
  .action((name: string) => executeModDisableCommand(name));

const listCommand = new Command('list')
  .description('List enabled mods')
  .action(() => executeModListCommand());

export const modCommand = new Command('mod')
  .description('Manage animaOS mods')
  .addCommand(enableCommand)
  .addCommand(disableCommand)
  .addCommand(listCommand);
