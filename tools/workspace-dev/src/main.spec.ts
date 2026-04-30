import { describe, expect, it } from 'vitest';

import { buildWorkspaceDevPlan, parseHostArg } from './main.js';

describe('workspace-dev orchestration', () => {
  it('parses --host from argv', () => {
    expect(parseHostArg(['--host', 'rust'])).toBe('rust');
  });

  it('plans the rust host and UI processes with injected backend origin', () => {
    expect(buildWorkspaceDevPlan('rust')).toEqual({
      host: {
        key: 'rust',
        projectName: 'rust-daemon',
        baseUrl: 'http://127.0.0.1:8080',
        status: 'ready',
        env: {
          ANIMAOS_RS_HOST: '127.0.0.1',
          ANIMAOS_RS_PORT: '8080',
        },
      },
      processes: [
        {
          name: 'rust-daemon',
          command: 'bun',
          args: ['x', 'nx', 'run', 'rust-daemon:dev'],
          env: {
            ANIMAOS_RS_HOST: '127.0.0.1',
            ANIMAOS_RS_PORT: '8080',
          },
        },
        {
          name: '@animaOS-SWARM/web',
          command: 'bun',
          args: ['x', 'nx', 'run', '@animaOS-SWARM/web:serve'],
          env: {
            UI_BACKEND_ORIGIN: 'http://127.0.0.1:8080',
            VITE_HOST_KEY: 'rust',
          },
        },
        {
          name: '@animaOS-SWARM/playground',
          command: 'bun',
          args: ['x', 'nx', 'run', '@animaOS-SWARM/playground:serve'],
          env: {
            UI_BACKEND_ORIGIN: 'http://127.0.0.1:8080',
            VITE_HOST_KEY: 'rust',
          },
        },
      ],
    });
  });

  it('reuses an already-running host and only starts the UI', () => {
    expect(
      buildWorkspaceDevPlan('rust', { reuseExistingHost: true })
    ).toEqual({
      host: {
        key: 'rust',
        projectName: 'rust-daemon',
        baseUrl: 'http://127.0.0.1:8080',
        status: 'ready',
        env: {
          ANIMAOS_RS_HOST: '127.0.0.1',
          ANIMAOS_RS_PORT: '8080',
        },
      },
      processes: [
        {
          name: '@animaOS-SWARM/web',
          command: 'bun',
          args: ['x', 'nx', 'run', '@animaOS-SWARM/web:serve'],
          env: {
            UI_BACKEND_ORIGIN: 'http://127.0.0.1:8080',
            VITE_HOST_KEY: 'rust',
          },
        },
        {
          name: '@animaOS-SWARM/playground',
          command: 'bun',
          args: ['x', 'nx', 'run', '@animaOS-SWARM/playground:serve'],
          env: {
            UI_BACKEND_ORIGIN: 'http://127.0.0.1:8080',
            VITE_HOST_KEY: 'rust',
          },
        },
      ],
    });
  });

  it('rejects placeholder hosts before any process starts', () => {
    expect(() => buildWorkspaceDevPlan('elixir')).toThrowError(
      "Host 'elixir' is registered as a placeholder and is not implemented yet."
    );
    expect(() => buildWorkspaceDevPlan('python')).toThrowError(
      "Host 'python' is registered as a placeholder and is not implemented yet."
    );
  });
});
