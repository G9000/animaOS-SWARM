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
        client.subscribe('/events', { signal: controller.signal }).next(),
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
        client.subscribe('/events', { signal: controller.signal }).next(),
      ).rejects.toThrow();
      expect(getEventListeners(controller.signal, 'abort')).toHaveLength(0);
    } finally {
      await heldReader.cancel();
      heldReader.releaseLock();
    }
  });
});

describe('daemon response bodies', () => {
  function failingBody(error: unknown): Response {
    return new Response(
      new ReadableStream({
        start(controller) {
          controller.error(error);
        },
      }),
      { status: 200, headers: { 'content-type': 'application/json' } },
    );
  }

  it('reports a body that fails after the headers as a connection error', async () => {
    const cause = new TypeError('network connection was lost');
    const client = createDaemonClient({
      fetch: async () => failingBody(cause),
    });

    const json = client.requestJson('/api/sessions');
    await expect(json).rejects.toBeInstanceOf(DaemonConnectionError);
    await expect(json).rejects.toMatchObject({ cause });
    await expect(client.requestText('/api/export')).rejects.toBeInstanceOf(
      DaemonConnectionError,
    );
  });

  it('keeps an abort while reading the body an abort', async () => {
    const abort = new DOMException('The operation was aborted.', 'AbortError');
    const client = createDaemonClient({
      fetch: async () => failingBody(abort),
    });

    await expect(client.requestJson('/api/sessions')).rejects.toMatchObject({
      name: 'AbortError',
    });
  });
});
