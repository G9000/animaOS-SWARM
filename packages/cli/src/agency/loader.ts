import { readFileSync, existsSync } from 'node:fs';
import { join } from 'node:path';
import yaml from 'js-yaml';
import type { AgencyConfig } from './types.js';

/**
 * Check whether an `anima.yaml` config file exists in the given directory.
 */
export function agencyExists(dir: string): boolean {
  return existsSync(join(dir, 'anima.yaml'));
}

/**
 * Load and validate an `anima.yaml` config file from the given directory.
 *
 * Applies sensible defaults for optional fields that are missing:
 * - `description` defaults to empty string
 * - `model` defaults to `"gpt-4o"`
 * - `provider` defaults to `"openai"`
 * - `strategy` defaults to `"supervisor"`
 * - `agents` defaults to empty array
 */
export function loadAgency(dir: string): AgencyConfig {
  const filePath = join(dir, 'anima.yaml');

  if (!existsSync(filePath)) {
    throw new Error(`Agency config not found: ${filePath}`);
  }

  const raw = readFileSync(filePath, 'utf-8');
  const parsed = yaml.load(raw) as Record<string, unknown>;

  if (!parsed || typeof parsed !== 'object') {
    throw new Error(
      `Invalid agency config: ${filePath} is not a valid YAML object`
    );
  }

  if (!parsed.name || typeof parsed.name !== 'string') {
    throw new Error('Agency config missing required field: name');
  }

  if (!parsed.orchestrator || typeof parsed.orchestrator !== 'object') {
    throw new Error('Agency config missing required field: orchestrator');
  }

  const orchestrator = parsed.orchestrator as Record<string, unknown>;
  if (!orchestrator.name || !orchestrator.bio || !orchestrator.system) {
    throw new Error(
      'Agency config orchestrator must have name, bio, and system fields'
    );
  }

  const agents = Array.isArray(parsed.agents) ? parsed.agents : [];

  function loadAgent(a: Record<string, unknown>) {
    return {
      name: (a.name as string) ?? 'unnamed',
      bio: (a.bio as string) ?? '',
      lore: a.lore as string | undefined,
      adjectives: Array.isArray(a.adjectives)
        ? (a.adjectives as string[])
        : undefined,
      topics: Array.isArray(a.topics) ? (a.topics as string[]) : undefined,
      knowledge: Array.isArray(a.knowledge)
        ? (a.knowledge as string[])
        : undefined,
      style: a.style as string | undefined,
      system: (a.system as string) ?? '',
      model: a.model as string | undefined,
      tools: a.tools as string[] | undefined,
    };
  }

  return {
    name: parsed.name as string,
    description: (parsed.description as string) ?? '',
    model: (parsed.model as string) ?? 'gpt-4o',
    provider: (parsed.provider as string) ?? 'openai',
    strategy: (parsed.strategy as AgencyConfig['strategy']) ?? 'supervisor',
    maxParallelDelegations:
      typeof parsed.maxParallelDelegations === 'number'
        ? parsed.maxParallelDelegations
        : undefined,
    orchestrator: loadAgent(orchestrator),
    agents: agents.map((a: Record<string, unknown>) => loadAgent(a)),
  };
}
