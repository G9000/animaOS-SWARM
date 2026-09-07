import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { expect, it } from 'vitest';
import { loadAgency } from './loader.js';

it('loads per-agent providers and preserves legacy inheritance', () => {
  const dir = mkdtempSync(join(tmpdir(), 'anima-loader-'));
  try {
    writeFileSync(join(dir, 'anima.yaml'), JSON.stringify({
      name: 'Team', provider: 'openai', model: 'default-model',
      orchestrator: { name: 'Manager', bio: 'Lead', system: 'Coordinate', provider: 'anthropic', model: 'lead-model' },
      agents: [{ name: 'Override', provider: 'google', model: 'worker-model' }, { name: 'Legacy' }],
    }));
    const agency = loadAgency(dir);
    expect(agency.orchestrator).toMatchObject({ provider: 'anthropic', model: 'lead-model' });
    expect(agency.agents[0]).toMatchObject({ provider: 'google', model: 'worker-model' });
    expect(agency.agents[1].provider).toBeUndefined();
    expect(agency).toMatchObject({ provider: 'openai', model: 'default-model' });
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
