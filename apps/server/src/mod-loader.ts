import { existsSync, readFileSync } from 'node:fs';
import { isAbsolute, join, relative, resolve } from 'node:path';
import type { ModPlugin } from '@animaOS-SWARM/core';
import { readModConfig } from '@animaOS-SWARM/mod-sdk';
import { registerModTools } from '@animaOS-SWARM/tools';

let modsLoaded = false;

/** For testing only */
export function resetModsLoaded(): void {
  modsLoaded = false;
}

interface ModManifest {
  name: string;
  version: string;
  description: string;
  main: string;
}

function isModManifest(value: unknown): value is ModManifest {
  return (
    typeof value === 'object' &&
    value !== null &&
    typeof (value as Record<string, unknown>)['name'] === 'string' &&
    typeof (value as Record<string, unknown>)['main'] === 'string' &&
    (value as Record<string, unknown>)['main'] !== ''
  );
}

function readManifest(modDir: string): ModManifest | null {
  const manifestPath = join(modDir, 'mod.json');
  if (!existsSync(manifestPath)) {
    process.stderr.write(`[mod-loader] No mod.json found at ${manifestPath}\n`);
    return null;
  }
  try {
    const parsed: unknown = JSON.parse(readFileSync(manifestPath, 'utf-8'));
    if (!isModManifest(parsed)) {
      process.stderr.write(`[mod-loader] Invalid mod.json at ${manifestPath}: missing or invalid "name"/"main" fields\n`);
      return null;
    }
    return parsed;
  } catch (err) {
    process.stderr.write(`[mod-loader] Failed to parse mod.json at ${manifestPath}: ${String(err)}\n`);
    return null;
  }
}

function findPlugin(module: unknown): ModPlugin | null {
  const candidate =
    module !== null && typeof module === 'object' && 'default' in module
      ? (module as { default: unknown }).default
      : null;

  if (
    candidate !== null &&
    typeof candidate === 'object' &&
    'tools' in candidate &&
    Array.isArray((candidate as Record<string, unknown>)['tools'])
  ) {
    return candidate as ModPlugin;
  }
  return null;
}

export async function loadEnabledMods(workspaceRoot: string): Promise<void> {
  if (modsLoaded) {
    process.stderr.write('[mod-loader] loadEnabledMods called more than once — skipping\n');
    return;
  }
  modsLoaded = true;

  const configPath = join(workspaceRoot, '.animaos', 'mods.json');
  const config = readModConfig(configPath);

  if (config.enabled.length === 0) return;

  for (const name of config.enabled) {
    // Guard against path traversal in mod names
    if (name.includes('..') || name.includes('/') || name.includes('\\')) {
      process.stderr.write(`[mod-loader] Skipping mod with unsafe name: "${name}"\n`);
      continue;
    }
    const modDir = join(workspaceRoot, 'mods', name);

    if (!existsSync(modDir)) {
      process.stderr.write(`[mod-loader] Mod directory not found: ${modDir}\n`);
      continue;
    }

    const manifest = readManifest(modDir);
    if (!manifest) continue;

    const mainPath = resolve(modDir, manifest.main);

    const rel = relative(modDir, mainPath);
    if (rel.startsWith('..') || isAbsolute(rel)) {
      process.stderr.write(`[mod-loader] Mod "${name}" main path escapes mod directory, skipping\n`);
      continue;
    }

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
