import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { readModConfig, writeModConfig } from '@animaOS-SWARM/mod-sdk';
import {
  executeModEnableCommand,
  executeModDisableCommand,
  executeModListCommand,
} from './mod.js';

function createTempDir(): string {
  return mkdtempSync(join(tmpdir(), 'animaos-mod-'));
}

describe('mod commands', () => {
  let tmpDir: string;
  let configFile: string;

  beforeEach(() => {
    tmpDir = createTempDir();
    configFile = join(tmpDir, 'mods.json');
  });

  afterEach(() => {
    vi.restoreAllMocks();
    rmSync(tmpDir, { recursive: true, force: true });
  });

  describe('enable', () => {
    it('adds a mod to a fresh config', () => {
      const logSpy = vi.spyOn(console, 'log').mockImplementation(() => {});

      executeModEnableCommand('crypto-trader', configFile);

      const config = readModConfig(configFile);
      expect(config.enabled).toEqual(['crypto-trader']);
      expect(logSpy).toHaveBeenCalledWith('✓ Enabled mod: crypto-trader');
    });

    it('is idempotent and does not duplicate an already enabled mod', () => {
      writeModConfig(configFile, { enabled: ['crypto-trader'] });
      const logSpy = vi.spyOn(console, 'log').mockImplementation(() => {});

      executeModEnableCommand('crypto-trader', configFile);

      const config = readModConfig(configFile);
      expect(config.enabled).toEqual(['crypto-trader']);
      expect(logSpy).toHaveBeenCalledWith('Mod "crypto-trader" is already enabled');
    });

    it('adds a second mod without removing the first', () => {
      writeModConfig(configFile, { enabled: ['crypto-trader'] });
      const logSpy = vi.spyOn(console, 'log').mockImplementation(() => {});

      executeModEnableCommand('another-mod', configFile);

      const config = readModConfig(configFile);
      expect(config.enabled).toEqual(['crypto-trader', 'another-mod']);
      expect(logSpy).toHaveBeenCalledWith('✓ Enabled mod: another-mod');
    });
  });

  describe('disable', () => {
    it('removes an enabled mod', () => {
      writeModConfig(configFile, { enabled: ['crypto-trader'] });
      const logSpy = vi.spyOn(console, 'log').mockImplementation(() => {});

      executeModDisableCommand('crypto-trader', configFile);

      const config = readModConfig(configFile);
      expect(config.enabled).toEqual([]);
      expect(logSpy).toHaveBeenCalledWith('✓ Disabled mod: crypto-trader');
    });

    it('is a no-op for a mod that is not enabled', () => {
      writeModConfig(configFile, { enabled: [] });
      const logSpy = vi.spyOn(console, 'log').mockImplementation(() => {});

      executeModDisableCommand('nonexistent-mod', configFile);

      const config = readModConfig(configFile);
      expect(config.enabled).toEqual([]);
      expect(logSpy).toHaveBeenCalledWith('Mod "nonexistent-mod" is not enabled');
    });

    it('removes only the specified mod and leaves others intact', () => {
      writeModConfig(configFile, { enabled: ['crypto-trader', 'another-mod'] });
      const logSpy = vi.spyOn(console, 'log').mockImplementation(() => {});

      executeModDisableCommand('crypto-trader', configFile);

      const config = readModConfig(configFile);
      expect(config.enabled).toEqual(['another-mod']);
      expect(logSpy).toHaveBeenCalledWith('✓ Disabled mod: crypto-trader');
    });
  });

  describe('list', () => {
    it('prints "No mods enabled" when no mods are enabled', () => {
      const logSpy = vi.spyOn(console, 'log').mockImplementation(() => {});

      executeModListCommand(configFile);

      expect(logSpy).toHaveBeenCalledWith('No mods enabled');
    });

    it('lists enabled mods with bullet points', () => {
      writeModConfig(configFile, { enabled: ['crypto-trader', 'another-mod'] });
      const logSpy = vi.spyOn(console, 'log').mockImplementation(() => {});

      executeModListCommand(configFile);

      expect(logSpy).toHaveBeenCalledWith('Enabled mods:');
      expect(logSpy).toHaveBeenCalledWith('  • crypto-trader');
      expect(logSpy).toHaveBeenCalledWith('  • another-mod');
    });

    it('returns the correct list after enabling mods', () => {
      writeModConfig(configFile, { enabled: ['mod-a', 'mod-b'] });
      const logSpy = vi.spyOn(console, 'log').mockImplementation(() => {});

      executeModListCommand(configFile);

      expect(logSpy).toHaveBeenCalledWith('Enabled mods:');
      expect(logSpy).toHaveBeenCalledWith('  • mod-a');
      expect(logSpy).toHaveBeenCalledWith('  • mod-b');
    });
  });
});
