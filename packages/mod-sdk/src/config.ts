import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname } from 'node:path';

export interface ModConfig {
  enabled: string[];
}

const DEFAULT_CONFIG: ModConfig = { enabled: [] };

export function readModConfig(configPath: string): ModConfig {
  if (!existsSync(configPath)) return { ...DEFAULT_CONFIG };
  try {
    return JSON.parse(readFileSync(configPath, 'utf-8')) as ModConfig;
  } catch {
    return { ...DEFAULT_CONFIG };
  }
}

export function writeModConfig(configPath: string, config: ModConfig): void {
  mkdirSync(dirname(configPath), { recursive: true });
  writeFileSync(configPath, JSON.stringify(config, null, 2) + '\n', 'utf-8');
}
