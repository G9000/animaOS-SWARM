import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname } from 'node:path';

export interface ModConfig {
  enabled: string[];
}

function defaultConfig(): ModConfig {
  return { enabled: [] };
}

function isModConfig(value: unknown): value is ModConfig {
  return (
    typeof value === 'object' &&
    value !== null &&
    'enabled' in value &&
    Array.isArray((value as Record<string, unknown>)['enabled'])
  );
}

export function readModConfig(configPath: string): ModConfig {
  if (!existsSync(configPath)) return defaultConfig();
  try {
    const parsed: unknown = JSON.parse(readFileSync(configPath, 'utf-8'));
    if (!isModConfig(parsed)) {
      process.stderr.write(`[mod-sdk] Invalid config shape at ${configPath}, using defaults\n`);
      return defaultConfig();
    }
    return parsed;
  } catch (err) {
    process.stderr.write(`[mod-sdk] Failed to parse config at ${configPath}: ${String(err)}\n`);
    return defaultConfig();
  }
}

export function writeModConfig(configPath: string, config: ModConfig): void {
  mkdirSync(dirname(configPath), { recursive: true });
  writeFileSync(configPath, JSON.stringify(config, null, 2) + '\n', 'utf-8');
}
