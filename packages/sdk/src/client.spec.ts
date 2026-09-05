import { getEventListeners } from 'node:events';
import { describe, expect, it } from 'vitest';

import { createDaemonClient, DaemonConnectionError } from './client.js';

describe('daemon subscription cleanup', () => {
  it('releases caller abort listeners after failed connection attempts', async () => {
    const controller = new AbortController();
    const client = createDaemonClient({
      fetch: async () => {
        throw new TypeError('connection refused');
      },
    });

    for (let attempt = 0; attempt < 3; attempt += 1) {
      await expect(
        client.subscribe('/events', { signal: controller.signal }).next()
      ).rejects.toBeInstanceOf(DaemonConnectionError);
      expect(getEventListeners(controller.signal, 'abort')).toHaveLength(0);
    }
  });

  it('releases caller abort listeners when the response body cannot be acquired', async () => {
    const controller = new AbortController();
    const response = new Response(new ReadableStream());
    const heldReader = response.body!.getReader();
    const client = createDaemonClient({ fetch: async () => response });

    try {
      await expect(
        client.subscribe('/events', { signal: controller.signal }).next()
      ).rejects.toThrow();
      expect(getEventListeners(controller.signal, 'abort')).toHaveLength(0);
    } finally {
      await heldReader.cancel();
      heldReader.releaseLock();
    }
  });
});
