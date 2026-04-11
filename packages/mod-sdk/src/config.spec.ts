import { writeFileSync } from 'node:fs';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { readModConfig, writeModConfig } from './config.js';

describe('readModConfig', () => {
  let tmpDir: string;

  beforeEach(() => {
    tmpDir = mkdtempSync(join(tmpdir(), 'mod-sdk-test-'));
  });

  afterEach(() => {
    rmSync(tmpDir, { recursive: true, force: true });
  });

  it('returns default config when file does not exist', () => {
    const result = readModConfig(join(tmpDir, 'mods.json'));
    expect(result).toEqual({ enabled: [] });
  });

  it('returns parsed config when file exists and is valid', () => {
    const configPath = join(tmpDir, 'mods.json');
    writeModConfig(configPath, { enabled: ['crypto-trader'] });
    const result = readModConfig(configPath);
    expect(result).toEqual({ enabled: ['crypto-trader'] });
  });

  it('returns default and logs to stderr when file contains invalid JSON', () => {
    const configPath = join(tmpDir, 'mods.json');
    writeFileSync(configPath, 'not-json');
    const stderrSpy = vi.spyOn(process.stderr, 'write').mockImplementation(() => true);
    try {
      const result = readModConfig(configPath);
      expect(result).toEqual({ enabled: [] });
      expect(stderrSpy).toHaveBeenCalledOnce();
    } finally {
      stderrSpy.mockRestore();
    }
  });

  it('returns default and logs to stderr when file has wrong shape', () => {
    const configPath = join(tmpDir, 'mods.json');
    writeFileSync(configPath, JSON.stringify({ enabled: 42 }));
    const stderrSpy = vi.spyOn(process.stderr, 'write').mockImplementation(() => true);
    try {
      const result = readModConfig(configPath);
      expect(result).toEqual({ enabled: [] });
      expect(stderrSpy).toHaveBeenCalledOnce();
    } finally {
      stderrSpy.mockRestore();
    }
  });
});

describe('writeModConfig', () => {
  let tmpDir: string;

  beforeEach(() => {
    tmpDir = mkdtempSync(join(tmpdir(), 'mod-sdk-test-'));
  });

  afterEach(() => {
    rmSync(tmpDir, { recursive: true, force: true });
  });

  it('writes config and creates directories', () => {
    const configPath = join(tmpDir, '.animaos', 'mods.json');
    writeModConfig(configPath, { enabled: ['crypto-trader', 'my-mod'] });
    const result = readModConfig(configPath);
    expect(result).toEqual({ enabled: ['crypto-trader', 'my-mod'] });
  });

  it('overwrites existing config', () => {
    const configPath = join(tmpDir, 'mods.json');
    writeModConfig(configPath, { enabled: ['old-mod'] });
    writeModConfig(configPath, { enabled: ['new-mod'] });
    const result = readModConfig(configPath);
    expect(result).toEqual({ enabled: ['new-mod'] });
  });
});
