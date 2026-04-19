import { describe, expect, it } from 'vitest';

import { normalizeVitestArgs } from './vitest-args.js';

describe('normalizeVitestArgs', () => {
  it('strips the Jest-only runInBand flag', () => {
    expect(normalizeVitestArgs(['--runInBand'])).toEqual([]);
  });

  it('strips runInBand when Nx passes it with an explicit value', () => {
    expect(normalizeVitestArgs(['--runInBand=true'])).toEqual([]);
  });

  it('strips runInBand when Nx passes its value as a separate argument', () => {
    expect(normalizeVitestArgs(['--runInBand', 'false'])).toEqual([]);
  });

  it('preserves all other Vitest arguments', () => {
    expect(
      normalizeVitestArgs([
        '--coverage',
        '--runInBand',
        '--watch',
        '--testNamePattern=workspace-dev',
      ])
    ).toEqual(['--coverage', '--watch', '--testNamePattern=workspace-dev']);
  });
});
