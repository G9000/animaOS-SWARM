import { afterEach, describe, expect, it, vi } from 'vitest';

import {
  createDaemonClient,
  LOG_LEVELS,
  MAX_LOGS_LIMIT,
  type LogEvent,
} from './index.js';

function sseResponse(chunks: string[]): Response {
  const encoder = new TextEncoder();
  return new Response(
    new ReadableStream({
      start(controller) {
        for (const chunk of chunks) controller.enqueue(encoder.encode(chunk));
        controller.close();
      },
    }),
    { headers: { 'content-type': 'text/event-stream' } },
  );
}

function line(seq: number, message = 'hello') {
  return {
    seq,
    at: 1000 + seq,
    level: 'info',
    target: 'anima_daemon',
    message,
  };
}

function logFrame(seq: number): string {
  return `event: log\ndata: ${JSON.stringify(line(seq))}\n\n`;
}

async function collect(stream: AsyncGenerator<LogEvent>): Promise<LogEvent[]> {
  const events: LogEvent[] = [];
  for await (const event of stream) events.push(event);
  return events;
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe('logs client', () => {
  it('list sends the filters', async () => {
    const requests: string[] = [];
    const client = createDaemonClient({
      baseUrl: '',
      fetch: async (url) => {
        requests.push(String(url));
        return Response.json({ lines: [line(1)], newestSeq: 1 });
      },
    });

    expect(
      await client.logs.list({ level: 'warn', q: 'a b', after: 0, limit: 50 }),
    ).toEqual({ lines: [line(1)], newestSeq: 1 });
    await client.logs.list();

    expect(requests).toEqual([
      '/api/logs?level=warn&q=a+b&after=0&limit=50',
      '/api/logs',
    ]);
  });

  it('stream yields line events in order', async () => {
    const client = createDaemonClient({
      baseUrl: '',
      fetch: async () =>
        sseResponse([logFrame(1), ': keep-alive\n\n', logFrame(2)]),
    });

    const events = await collect(client.logs.stream());

    expect(events).toEqual([
      { kind: 'line', line: line(1) },
      { kind: 'line', line: line(2) },
    ]);
  });

  it('stream yields a resync event with the newest seq', async () => {
    const client = createDaemonClient({
      baseUrl: '',
      fetch: async () =>
        sseResponse([logFrame(1), 'event: resync\ndata: {"newestSeq":42}\n\n']),
    });

    expect(await collect(client.logs.stream())).toEqual([
      { kind: 'line', line: line(1) },
      { kind: 'resync', newestSeq: 42 },
    ]);
  });

  it('stream skips malformed events with a warning', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    const client = createDaemonClient({
      baseUrl: '',
      fetch: async () =>
        sseResponse([
          'event: log\ndata: not json\n\n',
          'event: resync\ndata: {"newestSeq":"x"}\n\n',
          'event: other\ndata: {"seq":1}\n\n',
          logFrame(3),
        ]),
    });

    expect(await collect(client.logs.stream())).toEqual([
      { kind: 'line', line: line(3) },
    ]);
    expect(warn).toHaveBeenCalledTimes(3);
  });

  it('stream sends after, level, and q', async () => {
    const requests: string[] = [];
    const client = createDaemonClient({
      baseUrl: '',
      fetch: async (url) => {
        requests.push(String(url));
        return sseResponse([]);
      },
    });

    await collect(client.logs.stream({ level: 'debug', q: 'run', after: 7 }));
    await collect(client.logs.stream());

    expect(requests).toEqual([
      '/api/logs/stream?level=debug&q=run&after=7',
      '/api/logs/stream',
    ]);
  });

  it('stream stops when the signal aborts', async () => {
    const controller = new AbortController();
    const encoder = new TextEncoder();
    const client = createDaemonClient({
      baseUrl: '',
      fetch: async (_url, init) =>
        new Response(
          new ReadableStream({
            start(stream) {
              stream.enqueue(encoder.encode(logFrame(1)));
              init?.signal?.addEventListener('abort', () => {
                try {
                  stream.close();
                } catch {
                  // already closed
                }
              });
            },
          }),
          { headers: { 'content-type': 'text/event-stream' } },
        ),
    });

    const received: LogEvent[] = [];
    for await (const event of client.logs.stream({
      signal: controller.signal,
    })) {
      received.push(event);
      controller.abort();
    }

    expect(received).toEqual([{ kind: 'line', line: line(1) }]);
  });

  it('the log levels and limit match the daemon', () => {
    expect(LOG_LEVELS).toEqual(['error', 'warn', 'info', 'debug', 'trace']);
    expect(MAX_LOGS_LIMIT).toBe(1_000);
  });
});
