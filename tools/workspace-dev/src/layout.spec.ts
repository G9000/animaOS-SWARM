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

  it('stores reusable Rust crates under packages/core-rust', () => {
    expect(existsSync(repoPath('Cargo.toml'))).toBe(true);
    expect(
      existsSync(
        repoPath('packages', 'core-rust', 'crates', 'anima-core', 'Cargo.toml')
      )
    ).toBe(true);
    expect(
      existsSync(
        repoPath(
          'packages',
          'core-rust',
          'crates',
          'anima-memory',
          'Cargo.toml'
        )
      )
    ).toBe(true);
    expect(
      existsSync(
        repoPath(
          'packages',
          'core-rust',
          'crates',
          'anima-swarm',
          'Cargo.toml'
        )
      )
    ).toBe(true);

    expect(
      existsSync(repoPath('hosts', 'rust-daemon', 'Cargo.toml'))
    ).toBe(false);
    expect(
      existsSync(
        repoPath('hosts', 'rust-daemon', 'crates', 'anima-core', 'Cargo.toml')
      )
    ).toBe(false);
    expect(
      existsSync(
        repoPath(
          'hosts',
          'rust-daemon',
          'crates',
          'anima-memory',
          'Cargo.toml'
        )
      )
    ).toBe(false);
    expect(
      existsSync(
        repoPath(
          'hosts',
          'rust-daemon',
          'crates',
          'anima-swarm',
          'Cargo.toml'
        )
      )
    ).toBe(false);
  });
});
