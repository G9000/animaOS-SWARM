import { expect, it } from 'vitest';
import { daemonTestEnvironment } from './daemon-environment.js';

it('isolates the daemon from inherited persistence, credentials, and network settings', () => {
  const source = {
    Path: '/system/bin',
    SystemRoot: '/system',
    TMP: '/tmp',
    ANIMAOS_RS_CONTROL_PLANE_FILE: '/real/control.json',
    ANIMAOS_RS_MEMORY_SQLITE_FILE: '/real/memory.sqlite',
    ANIMAOS_WORKSPACE_ROOT: '/real/workspace',
    ANIMAOS_RS_API_KEY: 'real-daemon-key',
    ANIMA_LOCAL_ADMIN_TOKEN: 'real-owner-key',
    ANIMAOS_RS_PERSISTENCE_MODE: 'postgres',
    DATABASE_URL: 'real-db',
    OPENAI_API_KEY: 'real-provider-key',
    SOME_FUTURE_PROVIDER_KEY: 'secret',
    HTTP_PROXY: 'real-proxy',
  };
  expect(daemonTestEnvironment(source, '/fixture', 12345)).toEqual({
    Path: '/system/bin',
    SystemRoot: '/system',
    TMP: '/tmp',
    ANIMAOS_RS_HOST: '127.0.0.1',
    ANIMAOS_RS_PORT: '12345',
    ANIMAOS_WORKSPACE_ROOT: '/fixture',
    ANIMAOS_RS_PERSISTENCE_MODE: 'memory',
    ANIMAOS_RS_MEMORY_EMBEDDINGS: 'local',
  });
  expect(source.OPENAI_API_KEY).toBe('real-provider-key');
});
