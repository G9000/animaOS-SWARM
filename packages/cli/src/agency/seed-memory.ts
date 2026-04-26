import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import type { AgencyConfig, AgentDefinition } from './types.js';
import { agentSlug } from './diagram.js';

export type SeedMemoryType = 'fact' | 'observation' | 'task_result' | 'reflection';

export interface SeedMemoryEntry {
  type: SeedMemoryType;
  content: string;
  importance: number;
  tags?: string[];
}

export interface AgentSeedMemories {
  agentName: string;
  entries: SeedMemoryEntry[];
}

const VALID_TYPES = new Set<SeedMemoryType>([
  'fact',
  'observation',
  'task_result',
  'reflection',
]);

const DEFAULT_IMPORTANCE = 0.5;

/**
 * Discover and parse `agents/<slug>/memory/seed.json` for every agent in the
 * agency. Returns one entry per agent that has a non-empty seed file.
 *
 * Each entry in seed.json may look like:
 *   { "type": "fact", "content": "...", "importance": 0.8, "tags": ["..."] }
 *
 * `importance` defaults to 0.5 when omitted. `agentName` and `agentId` are
 * filled in at seed time — they don't belong in the file.
 */
export function loadSeedMemories(
  dir: string,
  agency: AgencyConfig
): AgentSeedMemories[] {
  const allAgents: AgentDefinition[] = [agency.orchestrator, ...agency.agents];
  const out: AgentSeedMemories[] = [];

  for (const agent of allAgents) {
    const seedPath = join(dir, 'agents', agentSlug(agent.name), 'memory', 'seed.json');
    if (!existsSync(seedPath)) continue;

    const raw = readFileSync(seedPath, 'utf-8').trim();
    if (!raw) continue;

    let parsed: unknown;
    try {
      parsed = JSON.parse(raw);
    } catch (error) {
      throw new Error(
        `Invalid seed memory for "${agent.name}" (${seedPath}): ${
          error instanceof Error ? error.message : String(error)
        }`
      );
    }

    const list = Array.isArray(parsed) ? parsed : [parsed];
    const entries = list.map((raw, index): SeedMemoryEntry => {
      if (!raw || typeof raw !== 'object') {
        throw new Error(
          `Seed memory entry ${index} for "${agent.name}" must be an object`
        );
      }
      const item = raw as Record<string, unknown>;

      const type = item.type;
      if (typeof type !== 'string' || !VALID_TYPES.has(type as SeedMemoryType)) {
        throw new Error(
          `Seed memory entry ${index} for "${agent.name}" has invalid type "${String(type)}". Valid: ${[
            ...VALID_TYPES,
          ].join(', ')}`
        );
      }

      const content = item.content;
      if (typeof content !== 'string' || !content.trim()) {
        throw new Error(
          `Seed memory entry ${index} for "${agent.name}" must have non-empty "content"`
        );
      }

      let importance = DEFAULT_IMPORTANCE;
      if (item.importance !== undefined) {
        if (typeof item.importance !== 'number' || !Number.isFinite(item.importance)) {
          throw new Error(
            `Seed memory entry ${index} for "${agent.name}" has non-numeric importance`
          );
        }
        if (item.importance < 0 || item.importance > 1) {
          throw new Error(
            `Seed memory entry ${index} for "${agent.name}" importance must be between 0 and 1`
          );
        }
        importance = item.importance;
      }

      const tags = Array.isArray(item.tags)
        ? item.tags.filter((t): t is string => typeof t === 'string')
        : undefined;

      return {
        type: type as SeedMemoryType,
        content,
        importance,
        ...(tags && tags.length > 0 ? { tags } : {}),
      };
    });

    if (entries.length > 0) {
      out.push({ agentName: agent.name, entries });
    }
  }

  return out;
}

interface DaemonAgentLike {
  state: { id: string; name: string };
}

/**
 * Resolve agent name → daemon agent ID using the swarm's agent IDs and the
 * daemon's per-agent fetch.
 */
export async function resolveAgentIds(
  agentIds: string[],
  fetchAgent: (agentId: string) => Promise<DaemonAgentLike>
): Promise<Map<string, string>> {
  const byName = new Map<string, string>();
  const snapshots = await Promise.all(
    agentIds.map((id) =>
      fetchAgent(id).catch(() => undefined as DaemonAgentLike | undefined)
    )
  );
  for (const snapshot of snapshots) {
    if (snapshot?.state?.name && snapshot.state.id) {
      byName.set(snapshot.state.name, snapshot.state.id);
    }
  }
  return byName;
}

interface MemoryPostBody {
  agentId: string;
  agentName: string;
  type: SeedMemoryType;
  content: string;
  importance: number;
  tags?: string[];
}

/**
 * Push every seed memory to the daemon. Failures are collected and returned
 * — they do not throw, so a bad seed entry never blocks a launch.
 */
export async function seedDaemonMemories(
  seeds: AgentSeedMemories[],
  agentIdsByName: Map<string, string>,
  postMemory: (body: MemoryPostBody) => Promise<unknown>
): Promise<{ created: number; errors: string[] }> {
  let created = 0;
  const errors: string[] = [];

  for (const seed of seeds) {
    const agentId = agentIdsByName.get(seed.agentName);
    if (!agentId) {
      errors.push(
        `Skipped seed memories for "${seed.agentName}" — agent not found in swarm`
      );
      continue;
    }

    for (const entry of seed.entries) {
      try {
        await postMemory({
          agentId,
          agentName: seed.agentName,
          type: entry.type,
          content: entry.content,
          importance: entry.importance,
          ...(entry.tags ? { tags: entry.tags } : {}),
        });
        created += 1;
      } catch (error) {
        errors.push(
          `Failed to seed memory for "${seed.agentName}": ${
            error instanceof Error ? error.message : String(error)
          }`
        );
      }
    }
  }

  return { created, errors };
}
