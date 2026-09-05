import { describe, expect, it } from 'vitest';
import {
  createDaemonClient,
  DaemonHttpError,
  type ConfigureOAuthAppInput,
  type OAuthAppStatus,
} from './index.js';

function transport(payload: unknown = {}) {
  const requests: { url: string; init?: RequestInit }[] = [];
  const client = createDaemonClient({
    baseUrl: '',
    fetch: async (url, init) => {
      requests.push({ url: String(url), init });
      return Response.json(payload);
    },
  });
  return { connectors: client.connectors, requests };
}

describe('connector transport', () => {
  it('gets, configures, and removes encoded OAuth app providers', async () => {
    const status: OAuthAppStatus = {
      provider: 'microsoft',
      configured: true,
      source: 'vault',
      clientIdHint: '...client',
      redirectUris: ['http://127.0.0.1:8080/oauth/callback'],
      tenant: 'organizations',
    };
    const input: ConfigureOAuthAppInput = {
      clientId: 'client-id',
      clientSecret: 'client-secret',
      tenant: 'organizations',
    };
    const { connectors, requests } = transport(status);

    expect(await connectors.oauthAppStatus('microsoft')).toEqual(status);
    expect(await connectors.configureOauthApp('microsoft', input)).toEqual(
      status,
    );
    await expect(
      connectors.removeOauthApp('microsoft'),
    ).resolves.toBeUndefined();

    expect(requests.map(({ url }) => url)).toEqual(
      Array(3).fill('/api/connectors/oauth-apps/microsoft'),
    );
    expect(requests.map(({ init }) => init?.method)).toEqual([
      undefined,
      'PUT',
      'DELETE',
    ]);
    expect(JSON.parse(requests[1].init?.body as string)).toEqual(input);
  });

  it('does not retry OAuth app configuration failures', async () => {
    let count = 0;
    const client = createDaemonClient({
      baseUrl: '',
      fetch: async () => {
        count += 1;
        return Response.json({ error: 'vault_unavailable' }, { status: 503 });
      },
    });

    await expect(
      client.connectors.configureOauthApp('google', {
        clientId: 'client-id',
        clientSecret: 'client-secret',
      }),
    ).rejects.toMatchObject({
      name: 'DaemonHttpError',
      status: 503,
      message: 'vault_unavailable',
    } satisfies Partial<DaemonHttpError>);
    expect(count).toBe(1);
  });

  it('explicitly refreshes cached mail only when requested', async () => {
    const { connectors, requests } = transport({ messages: [] });
    await connectors.mailMessages('a', 'gmail', 'c', { refresh: true });
    await connectors.mailMessages('a', 'gmail', 'c', { refresh: false });
    expect(requests.map(({ url }) => url)).toEqual([
      '/api/agents/a/connectors/mail/gmail/c/messages?refresh=true',
      '/api/agents/a/connectors/mail/gmail/c/messages',
    ]);
  });
  it.each(['gmail', 'outlook'] as const)(
    'preserves %s status and consent envelopes over same-origin transport',
    async (provider) => {
      const payload = {
        configured: true,
        connector: null,
        consentUrl: 'https://consent.example',
      };
      const { connectors, requests } = transport(payload);
      expect(await connectors.mailStatus('agent /?', provider)).toEqual(
        payload,
      );
      expect(await connectors.connectMail('agent /?', provider)).toEqual(
        payload,
      );
      expect(requests.map(({ url }) => url)).toEqual(
        Array(2).fill(`/api/agents/agent%20%2F%3F/connectors/mail/${provider}`),
      );
      expect(requests[1].init?.method).toBe('POST');
    },
  );

  it('encodes mail identifiers and keeps draft creation separate from send approval', async () => {
    const draft = { id: 'draft /?', state: 'pending' };
    const input = {
      to: ['owner@example.com'],
      subject: 'Review',
      body: 'Text',
    };
    const { connectors, requests } = transport({
      draft,
      drafts: [draft],
      messages: [{ id: 'm' }],
    });
    expect(await connectors.mailMessages('a', 'gmail', 'c /?')).toEqual([
      { id: 'm' },
    ]);
    expect(await connectors.mailDrafts('a', 'gmail', 'c /?')).toEqual([draft]);
    expect(
      await connectors.createMailDraft('a', 'gmail', 'c /?', input),
    ).toEqual(draft);
    expect(requests).toHaveLength(3);
    expect(JSON.parse(requests[2].init?.body as string)).toEqual(input);
    expect(
      await connectors.approveMailDraft('a', 'gmail', 'c /?', 'd /?'),
    ).toEqual(draft);
    expect(
      await connectors.rejectMailDraft('a', 'gmail', 'c /?', 'd /?'),
    ).toEqual(draft);
    await connectors.disconnectMail('a', 'gmail', 'c /?');
    const prefix = '/api/agents/a/connectors/mail/gmail/c%20%2F%3F';
    expect(requests.map(({ url }) => url)).toEqual([
      `${prefix}/messages`,
      `${prefix}/drafts`,
      `${prefix}/drafts`,
      `${prefix}/drafts/d%20%2F%3F/approve`,
      `${prefix}/drafts/d%20%2F%3F/reject`,
      prefix,
    ]);
    expect(requests.slice(2).map(({ init }) => init?.method)).toEqual([
      'POST',
      'POST',
      'POST',
      'DELETE',
    ]);
  });

  it('matches Calendar route envelopes and approval paths', async () => {
    const write = { id: 'w', state: 'applied' };
    const payload = {
      configured: true,
      connector: null,
      consentUrl: 'https://consent.example',
      writes: [write],
      write,
    };
    const { connectors, requests } = transport(payload);
    expect(await connectors.calendarStatus('a /?')).toEqual(payload);
    expect(await connectors.connectCalendar('a /?')).toEqual(payload);
    expect(await connectors.calendarWrites('a /?', 'c /?')).toEqual([write]);
    expect(
      await connectors.approveCalendarWrite('a /?', 'c /?', 'w /?'),
    ).toEqual(write);
    expect(
      await connectors.rejectCalendarWrite('a /?', 'c /?', 'w /?'),
    ).toEqual(write);
    await connectors.disconnectCalendar('a /?', 'c /?');
    const prefix = '/api/agents/a%20%2F%3F/connectors/gcalendar';
    expect(requests.map(({ url }) => url)).toEqual([
      prefix,
      prefix,
      `${prefix}/c%20%2F%3F/writes`,
      `${prefix}/c%20%2F%3F/writes/w%20%2F%3F/approve`,
      `${prefix}/c%20%2F%3F/writes/w%20%2F%3F/reject`,
      `${prefix}/c%20%2F%3F`,
    ]);
    expect(
      [requests[1], ...requests.slice(3)].map(({ init }) => init?.method),
    ).toEqual(['POST', 'POST', 'POST', 'DELETE']);
  });

  it('propagates owner rejection without retrying a send', async () => {
    let count = 0;
    const client = createDaemonClient({
      baseUrl: '/daemon/',
      fetch: async (url) => {
        count += 1;
        expect(String(url)).toBe(
          '/daemon/api/agents/a/connectors/mail/outlook/c/drafts/d/approve',
        );
        return Response.json(
          { error: 'local_owner_required' },
          { status: 403 },
        );
      },
    });
    await expect(
      client.connectors.approveMailDraft('a', 'outlook', 'c', 'd'),
    ).rejects.toMatchObject({
      name: 'DaemonHttpError',
      status: 403,
      message: 'local_owner_required',
    } satisfies Partial<DaemonHttpError>);
    expect(count).toBe(1);
  });
});
