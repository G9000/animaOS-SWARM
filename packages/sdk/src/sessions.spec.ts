import { describe, expect, it } from 'vitest';

import {
  createDaemonClient,
  DaemonHttpError,
  DaemonTooOldError,
} from './index.js';

function transport(respond: (url: string) => Response) {
  const requests: { url: string; init?: RequestInit }[] = [];
  const client = createDaemonClient({
    baseUrl: '',
    fetch: async (url, init) => {
      requests.push({ url: String(url), init });
      return respond(String(url));
    },
  });
  return { sessions: client.sessions, requests };
}

const session = {
  id: 'chat:1',
  agentId: 'agent/a',
  roomId: 'chat:1',
  kind: 'chat',
  title: 'Plans',
};

describe('sessions client', () => {
  it('lists sessions with encoded filters', async () => {
    const page = { sessions: [session], nextCursor: 'next' };
    const { sessions, requests } = transport(() => Response.json(page));

    expect(
      await sessions.list('agent/a', {
        kind: 'chat',
        archived: false,
        q: 'budget plan',
        cursor: 'abc',
        limit: 20,
        includeHelpers: false,
      }),
    ).toEqual(page);
    await sessions.list('agent/a');

    expect(requests.map(({ url }) => url)).toEqual([
      '/api/agents/agent%2Fa/sessions?kind=chat&archived=false&q=budget+plan&cursor=abc&limit=20&includeHelpers=false',
      '/api/agents/agent%2Fa/sessions',
    ]);
  });

  it('reads one session and a message page with an encoded session id', async () => {
    const { sessions, requests } = transport((url) =>
      url.includes('/messages')
        ? Response.json({ messages: [{ id: 'm1' }], nextBefore: 'm1' })
        : Response.json({ session }),
    );

    expect(await sessions.get('agent/a', 'chat:1')).toEqual(session);
    expect(
      await sessions.messages('agent/a', 'chat:1', {
        before: 'm2',
        limit: 10,
        includeHidden: true,
      }),
    ).toEqual({ messages: [{ id: 'm1' }], nextBefore: 'm1' });
    await sessions.messages('agent/a', 'chat:1');

    expect(requests.map(({ url }) => url)).toEqual([
      '/api/agents/agent%2Fa/sessions/chat%3A1',
      '/api/agents/agent%2Fa/sessions/chat%3A1/messages?before=m2&limit=10&includeHidden=true',
      '/api/agents/agent%2Fa/sessions/chat%3A1/messages',
    ]);
  });

  it('creates, updates, and removes sessions', async () => {
    const { sessions, requests } = transport((url) =>
      url.endsWith('chat%3A1') && requests.at(-1)?.init?.method === 'DELETE'
        ? Response.json({ deleted: true })
        : Response.json({ session }),
    );

    expect(await sessions.create('agent/a', { title: 'Trip' })).toEqual(
      session,
    );
    expect(await sessions.create('agent/a')).toEqual(session);
    expect(
      await sessions.update('agent/a', 'chat:1', {
        archived: true,
        lastReadAtMs: 5,
      }),
    ).toEqual(session);
    await expect(sessions.remove('agent/a', 'chat:1')).resolves.toBeUndefined();

    expect(
      requests.map(({ url, init }) => [
        init?.method,
        url,
        init?.body === undefined ? undefined : JSON.parse(String(init.body)),
      ]),
    ).toEqual([
      ['POST', '/api/agents/agent%2Fa/sessions', { title: 'Trip' }],
      ['POST', '/api/agents/agent%2Fa/sessions', {}],
      [
        'PATCH',
        '/api/agents/agent%2Fa/sessions/chat%3A1',
        { archived: true, lastReadAtMs: 5 },
      ],
      ['DELETE', '/api/agents/agent%2Fa/sessions/chat%3A1', undefined],
    ]);
  });

  it('exports Markdown as text and surfaces daemon errors', async () => {
    const { sessions, requests } = transport((url) =>
      url.includes('missing')
        ? Response.json({ error: 'not found' }, { status: 404 })
        : new Response('# Plans\n', {
            headers: { 'content-type': 'text/markdown; charset=utf-8' },
          }),
    );

    expect(await sessions.exportMarkdown('agent/a', 'chat:1')).toBe(
      '# Plans\n',
    );
    expect(requests[0].url).toBe(
      '/api/agents/agent%2Fa/sessions/chat%3A1/export',
    );
    expect(
      (requests[0].init?.headers as Record<string, string>).accept,
    ).toContain('text/markdown');

    const failure = sessions.exportMarkdown('agent/a', 'chat:missing');
    await expect(failure).rejects.toBeInstanceOf(DaemonHttpError);
    await expect(failure).rejects.toMatchObject({
      status: 404,
      message: 'not found',
    });
  });

  // Controller ruling 2 (M2 pre-flight audit): a 404 from the sessions route
  // is ambiguous by itself (an unknown route and a real not-found answer the
  // same JSON 404), so `list` alone probes `GET /api/agents/{agentId}` to
  // tell "daemon too old" apart from "agent not found".
  it('raises a too-old error when sessions 404s but the agent exists', async () => {
    const { sessions, requests } = transport((url) =>
      url === '/api/agents/agent%2Fa/sessions'
        ? Response.json({ error: 'not found' }, { status: 404 })
        : Response.json({ agent: { state: { id: 'agent/a' } } }),
    );

    const failure = sessions.list('agent/a');
    await expect(failure).rejects.toBeInstanceOf(DaemonTooOldError);
    await expect(failure).rejects.toMatchObject({ code: 'daemon_too_old' });
    expect(requests.map(({ url }) => url)).toEqual([
      '/api/agents/agent%2Fa/sessions',
      '/api/agents/agent%2Fa',
    ]);
  });

  it('keeps the original not-found when both the sessions route and the agent 404', async () => {
    const { sessions, requests } = transport(() =>
      Response.json({ error: 'not found' }, { status: 404 }),
    );

    const failure = sessions.list('agent/missing');
    await expect(failure).rejects.toBeInstanceOf(DaemonHttpError);
    await expect(failure).rejects.not.toBeInstanceOf(DaemonTooOldError);
    await expect(failure).rejects.toMatchObject({
      status: 404,
      message: 'not found',
    });
    expect(requests.map(({ url }) => url)).toEqual([
      '/api/agents/agent%2Fmissing/sessions',
      '/api/agents/agent%2Fmissing',
    ]);
  });
});
