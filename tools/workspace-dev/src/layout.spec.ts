import { existsSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

function repoPath(...segments: string[]) {
  return resolve(import.meta.dirname, '..', '..', '..', ...segments);
}

describe('repo layout', () => {
  it('stores the TypeScript core port in packages/core-ts', () => {
    expect(existsSync(repoPath('packages', 'core-ts', 'package.json'))).toBe(
      true
    );
    expect(existsSync(repoPath('packages', 'core', 'package.json'))).toBe(
      false
    );
  });
});
