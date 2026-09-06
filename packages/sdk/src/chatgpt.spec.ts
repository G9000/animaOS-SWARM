import { describe, expect, it } from 'vitest';
import {
  createDaemonClient,
  DaemonHttpError,
  type ChatGptStatus,
} from './index.js';

describe('ChatGPT subscription transport', () => {
  it('uses the redacted lifecycle endpoints without caching or credentials in bodies', async () => {
    const calls: { url: string; init?: RequestInit }[] = [];
    const status: ChatGptStatus = {
      connected: false,
      accountId: null,
      planType: null,
      login: null,
      error: null,
    };
    const client = createDaemonClient({
      baseUrl: '',
      fetch: async (url, init) => {
        calls.push({ url: String(url), init });
        return Response.json(status);
      },
    });
    for (const action of ['status', 'login', 'cancel', 'disconnect'] as const) {
      expect(await client.chatgpt[action]()).toEqual(status);
    }
    expect(
      calls.map(({ url, init }) => [
        url,
        init?.method ?? 'GET',
        (init?.headers as Record<string, string>)['cache-control'],
        init?.body,
      ]),
    ).toEqual([
      ['/api/providers/chatgpt/status', 'GET', 'no-store', undefined],
      ['/api/providers/chatgpt/login', 'POST', 'no-store', undefined],
      ['/api/providers/chatgpt/login', 'DELETE', 'no-store', undefined],
      ['/api/providers/chatgpt', 'DELETE', 'no-store', undefined],
    ]);
  });

  it('surfaces host failures', async () => {
    const client = createDaemonClient({
      fetch: async () =>
        Response.json({ error: 'vault_unavailable' }, { status: 503 }),
    });
    await expect(client.chatgpt.disconnect()).rejects.toBeInstanceOf(
      DaemonHttpError,
    );
  });
});
