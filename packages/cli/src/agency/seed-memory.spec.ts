import { describe, expect, it } from 'vitest';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import {
  loadSeedMemories,
  resolveAgentIds,
  seedDaemonMemories,
} from './seed-memory.js';
import type { AgencyConfig } from './types.js';

function makeAgency(): AgencyConfig {
  return {
    name: 'Test Agency',
    description: 'fixture',
    model: 'gpt-4o',
    provider: 'openai',
    strategy: 'supervisor',
    orchestrator: {
      name: 'Sarah Chen',
      bio: 'orchestrator',
      system: 'lead',
    },
    agents: [
      { name: 'Marcus Rivera', bio: 'worker', system: 'execute' },
      { name: 'Aiko Tanaka', bio: 'worker', system: 'execute' },
    ],
  };
}

function writeSeed(dir: string, slug: string, body: unknown) {
  const memoryDir = join(dir, 'agents', slug, 'memory');
  mkdirSync(memoryDir, { recursive: true });
  writeFileSync(join(memoryDir, 'seed.json'), JSON.stringify(body));
}

describe('loadSeedMemories', () => {
  it('returns empty list when no seed files exist', () => {
    const dir = mkdtempSync(join(tmpdir(), 'animaos-seed-'));
    try {
      expect(loadSeedMemories(dir, makeAgency())).toEqual([]);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it('reads valid seed entries and applies default importance', () => {
    const dir = mkdtempSync(join(tmpdir(), 'animaos-seed-'));
    try {
      writeSeed(dir, 'sarah-chen', [
        { type: 'fact', content: 'water is wet' },
        { type: 'observation', content: 'sky is blue', importance: 0.8, tags: ['nature'] },
      ]);
      const result = loadSeedMemories(dir, makeAgency());
      expect(result).toEqual([
        {
          agentName: 'Sarah Chen',
          entries: [
            { type: 'fact', content: 'water is wet', importance: 0.5 },
            { type: 'observation', content: 'sky is blue', importance: 0.8, tags: ['nature'] },
          ],
        },
      ]);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it('accepts a single object instead of an array', () => {
    const dir = mkdtempSync(join(tmpdir(), 'animaos-seed-'));
    try {
      writeSeed(dir, 'marcus-rivera', { type: 'fact', content: 'solo entry' });
      const result = loadSeedMemories(dir, makeAgency());
      expect(result).toHaveLength(1);
      expect(result[0].agentName).toBe('Marcus Rivera');
      expect(result[0].entries[0].content).toBe('solo entry');
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it('throws on invalid memory type', () => {
    const dir = mkdtempSync(join(tmpdir(), 'animaos-seed-'));
    try {
      writeSeed(dir, 'sarah-chen', [{ type: 'bogus', content: 'x' }]);
      expect(() => loadSeedMemories(dir, makeAgency())).toThrow(/invalid type/);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it('throws on out-of-range importance', () => {
    const dir = mkdtempSync(join(tmpdir(), 'animaos-seed-'));
    try {
      writeSeed(dir, 'sarah-chen', [
        { type: 'fact', content: 'x', importance: 2 },
      ]);
      expect(() => loadSeedMemories(dir, makeAgency())).toThrow(/between 0 and 1/);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it('throws on malformed JSON with helpful path', () => {
    const dir = mkdtempSync(join(tmpdir(), 'animaos-seed-'));
    try {
      const memoryDir = join(dir, 'agents', 'sarah-chen', 'memory');
      mkdirSync(memoryDir, { recursive: true });
      writeFileSync(join(memoryDir, 'seed.json'), '{ not json');
      expect(() => loadSeedMemories(dir, makeAgency())).toThrow(/Sarah Chen/);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});

describe('resolveAgentIds', () => {
  it('maps agent names to ids using the daemon fetcher', async () => {
    const fetchAgent = async (id: string) => ({
      state: {
        id,
        name: id === 'id-a' ? 'Sarah Chen' : 'Marcus Rivera',
      },
    });

    const map = await resolveAgentIds(['id-a', 'id-b'], fetchAgent);
    expect(map.get('Sarah Chen')).toBe('id-a');
    expect(map.get('Marcus Rivera')).toBe('id-b');
  });

  it('skips agent ids whose fetch fails', async () => {
    const fetchAgent = async (id: string) => {
      if (id === 'id-a') throw new Error('not found');
      return { state: { id, name: 'Aiko Tanaka' } };
    };

    const map = await resolveAgentIds(['id-a', 'id-b'], fetchAgent);
    expect(map.size).toBe(1);
    expect(map.get('Aiko Tanaka')).toBe('id-b');
  });
});

describe('seedDaemonMemories', () => {
  it('posts every entry with the resolved agent id', async () => {
    const calls: Array<Record<string, unknown>> = [];
    const result = await seedDaemonMemories(
      [
        {
          agentName: 'Sarah Chen',
          entries: [
            { type: 'fact', content: 'a', importance: 0.5 },
            { type: 'observation', content: 'b', importance: 0.7, tags: ['x'] },
          ],
        },
      ],
      new Map([['Sarah Chen', 'agent-1']]),
      async (body) => {
        calls.push(body as unknown as Record<string, unknown>);
      }
    );

    expect(result.created).toBe(2);
    expect(result.errors).toEqual([]);
    expect(calls).toHaveLength(2);
    expect(calls[0]).toMatchObject({
      agentId: 'agent-1',
      agentName: 'Sarah Chen',
      type: 'fact',
      content: 'a',
      importance: 0.5,
    });
    expect(calls[1]).toMatchObject({ tags: ['x'] });
  });

  it('records an error when an agent name has no resolved id', async () => {
    const calls: Array<unknown> = [];
    const result = await seedDaemonMemories(
      [
        {
          agentName: 'Ghost',
          entries: [{ type: 'fact', content: 'a', importance: 0.5 }],
        },
      ],
      new Map(),
      async (body) => {
        calls.push(body);
      }
    );

    expect(result.created).toBe(0);
    expect(calls).toEqual([]);
    expect(result.errors[0]).toMatch(/Ghost/);
  });

  it('continues past per-entry post failures and reports them', async () => {
    let count = 0;
    const result = await seedDaemonMemories(
      [
        {
          agentName: 'Sarah Chen',
          entries: [
            { type: 'fact', content: 'a', importance: 0.5 },
            { type: 'fact', content: 'b', importance: 0.5 },
          ],
        },
      ],
      new Map([['Sarah Chen', 'agent-1']]),
      async () => {
        count += 1;
        if (count === 1) throw new Error('boom');
      }
    );

    expect(result.created).toBe(1);
    expect(result.errors).toHaveLength(1);
    expect(result.errors[0]).toMatch(/boom/);
  });
});
