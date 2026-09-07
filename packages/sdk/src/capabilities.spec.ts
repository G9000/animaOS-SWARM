import { expect, it } from 'vitest';
import { createDaemonClient, DaemonHttpError } from './index.js';

it('reads the daemon capability inventory without changing permissions', async () => {
  const calls: Array<[string, string | undefined]> = [];
  const inventory = { schemaVersion: 1, tools: [], persistence: { controlPlane: 'sqlite', memory: 'sqlite', executionJournal: false }, extensions: [], limitations: ['Registered tools still require access.'] };
  const client = createDaemonClient({ baseUrl: '', fetch: async (url, init) => {
    calls.push([String(url), init?.method]);
    return Response.json(inventory);
  } });
  expect(await client.capabilities()).toEqual(inventory);
  expect(calls).toEqual([['/api/capabilities', undefined]]);
});

it('preserves authorization errors from capability discovery', async () => {
  const client = createDaemonClient({ baseUrl: '', fetch: async () => Response.json({ error: 'local owner authorization required' }, { status: 403 }) });
  await expect(client.capabilities()).rejects.toBeInstanceOf(DaemonHttpError);
});
