import { describe, expect, it } from 'vitest';

import { HOST_KEYS, getHostDefinition, isHostKey } from './hosts.js';

describe('workspace-dev host registry', () => {
  it('recognizes the supported host keys', () => {
    expect(HOST_KEYS).toEqual(['rust', 'elixir', 'python']);
    expect(isHostKey('rust')).toBe(true);
    expect(isHostKey('elixir')).toBe(true);
    expect(isHostKey('python')).toBe(true);
  });

  it('marks rust as the ready host project', () => {
    expect(getHostDefinition('rust')).toEqual({
      key: 'rust',
      projectName: 'rust-daemon',
      baseUrl: 'http://127.0.0.1:8080',
      status: 'ready',
      env: {
        ANIMAOS_RS_HOST: '127.0.0.1',
        ANIMAOS_RS_PORT: '8080',
      },
    });
  });

  it('marks elixir and python as placeholders', () => {
    expect(getHostDefinition('elixir')).toMatchObject({
      key: 'elixir',
      projectName: 'elixir-phoenix',
      status: 'placeholder',
    });
    expect(getHostDefinition('python')).toMatchObject({
      key: 'python',
      projectName: 'python-service',
      status: 'placeholder',
    });
  });

  it('rejects unknown hosts with a clear error', () => {
    expect(() => getHostDefinition('go')).toThrowError(
      "Unknown host 'go'. Expected one of: rust, elixir, python."
    );
  });
});
