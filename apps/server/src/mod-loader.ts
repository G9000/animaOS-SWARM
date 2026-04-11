import { existsSync, readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import type { ModPlugin } from '@animaOS-SWARM/core';
import { readModConfig } from '@animaOS-SWARM/mod-sdk';
import { registerModTools } from '@animaOS-SWARM/tools';

interface ModManifest {
  name: string;
  version: string;
  description: string;
  main: string;
}

function readManifest(modDir: string): ModManifest | null {
  const manifestPath = join(modDir, 'mod.json');
  if (!existsSync(manifestPath)) {
    process.stderr.write(`[mod-loader] No mod.json found at ${manifestPath}\n`);
    return null;
  }
  try {
    return JSON.parse(readFileSync(manifestPath, 'utf-8')) as ModManifest;
  } catch (err) {
    process.stderr.write(`[mod-loader] Failed to parse mod.json at ${manifestPath}: ${String(err)}\n`);
    return null;
  }
}

function findPlugin(module: unknown): ModPlugin | null {
  if (
    module !== null &&
    typeof module === 'object' &&
    'default' in module &&
    typeof (module as { default: unknown }).default === 'object'
  ) {
    return (module as { default: ModPlugin }).default;
  }
  return null;
}

export async function loadEnabledMods(workspaceRoot: string): Promise<void> {
  const configPath = join(workspaceRoot, '.animaos', 'mods.json');
  const config = readModConfig(configPath);

  if (config.enabled.length === 0) return;

  for (const name of config.enabled) {
    const modDir = join(workspaceRoot, 'mods', name);

    if (!existsSync(modDir)) {
      process.stderr.write(`[mod-loader] Mod directory not found: ${modDir}\n`);
      continue;
    }

    const manifest = readManifest(modDir);
    if (!manifest) continue;

    const mainPath = resolve(modDir, manifest.main);

    try {
      const module = await import(mainPath);
      const plugin = findPlugin(module);

      if (!plugin) {
        process.stderr.write(`[mod-loader] Mod "${name}" has no valid default ModPlugin export\n`);
        continue;
      }

      registerModTools(plugin.tools);
      process.stdout.write(`[mod-loader] Loaded mod "${name}" with ${plugin.tools.length} tool(s)\n`);
    } catch (err) {
      process.stderr.write(`[mod-loader] Failed to load mod "${name}": ${String(err)}\n`);
    }
  }
}
