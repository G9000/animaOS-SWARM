import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { loadEnabledMods, resetModsLoaded } from './mod-loader.js';
import { MOD_TOOL_MAP } from '@animaOS-SWARM/tools';

function writeMod(modsDir: string, name: string, toolName: string): void {
  const modDir = join(modsDir, name);
  mkdirSync(modDir, { recursive: true });
  // mod.json manifest
  writeFileSync(join(modDir, 'mod.json'), JSON.stringify({
    name,
    version: '1.0.0',
    description: 'test mod',
    main: 'index.js',
  }));
  // Simple CJS-compatible index.js (works under both Bun and Node)
  writeFileSync(join(modDir, 'index.js'), `
    exports.default = {
      name: '${name}',
      description: 'test',
      tools: [{
        name: '${toolName}',
        description: 'test tool',
        parameters: { type: 'object', properties: {} },
        execute: async () => ({ ok: true }),
      }],
    };
  `);
}

describe('loadEnabledMods', () => {
  let tmpDir: string;

  beforeEach(() => {
    tmpDir = mkdtempSync(join(tmpdir(), 'mod-loader-test-'));
    resetModsLoaded();
    MOD_TOOL_MAP.clear();
  });

  afterEach(() => {
    rmSync(tmpDir, { recursive: true, force: true });
    MOD_TOOL_MAP.clear();
    resetModsLoaded();
  });

  it('registers tools from enabled mods', async () => {
    // Write .animaos/mods.json
    const animaosDir = join(tmpDir, '.animaos');
    mkdirSync(animaosDir, { recursive: true });
    writeFileSync(join(animaosDir, 'mods.json'), JSON.stringify({ enabled: ['test-mod'] }));
    // Write mod files
    writeMod(join(tmpDir, 'mods'), 'test-mod', 'my_tool');

    await loadEnabledMods(tmpDir);

    expect(MOD_TOOL_MAP.has('my_tool')).toBe(true);
  });

  it('does nothing when no mods are enabled', async () => {
    const animaosDir = join(tmpDir, '.animaos');
    mkdirSync(animaosDir, { recursive: true });
    writeFileSync(join(animaosDir, 'mods.json'), JSON.stringify({ enabled: [] }));

    await loadEnabledMods(tmpDir);

    expect(MOD_TOOL_MAP.size).toBe(0);
  });

  it('skips missing mod directory gracefully', async () => {
    const animaosDir = join(tmpDir, '.animaos');
    mkdirSync(animaosDir, { recursive: true });
    writeFileSync(join(animaosDir, 'mods.json'), JSON.stringify({ enabled: ['nonexistent-mod'] }));

    await expect(loadEnabledMods(tmpDir)).resolves.not.toThrow();
    expect(MOD_TOOL_MAP.size).toBe(0);
  });

  it('is idempotent — second call is a no-op', async () => {
    const animaosDir = join(tmpDir, '.animaos');
    mkdirSync(animaosDir, { recursive: true });
    writeFileSync(join(animaosDir, 'mods.json'), JSON.stringify({ enabled: ['test-mod'] }));
    writeMod(join(tmpDir, 'mods'), 'test-mod', 'my_tool');

    await loadEnabledMods(tmpDir);
    await loadEnabledMods(tmpDir); // second call should be skipped

    expect(MOD_TOOL_MAP.size).toBe(1); // not doubled
  });

  it('skips mods with unsafe names (path traversal)', async () => {
    const animaosDir = join(tmpDir, '.animaos');
    mkdirSync(animaosDir, { recursive: true });
    writeFileSync(join(animaosDir, 'mods.json'), JSON.stringify({ enabled: ['../evil'] }));

    await loadEnabledMods(tmpDir);

    expect(MOD_TOOL_MAP.size).toBe(0);
  });
});
