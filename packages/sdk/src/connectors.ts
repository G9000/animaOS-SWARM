import type { DaemonClient } from './client.js';

export type ConnectorStatus = 'pairing' | 'active' | 'reauthRequired';
export type MailProvider = 'gmail' | 'outlook';
export type OAuthAppProvider = 'google' | 'microsoft';

export interface OAuthAppStatus {
  provider: OAuthAppProvider;
  configured: boolean;
  source: 'vault' | 'environment' | null;
  clientIdHint: string | null;
  redirectUris: string[];
  tenant: string | null;
}

export interface ConfigureOAuthAppInput {
  clientId: string;
  clientSecret: string;
  tenant?: string;
}

export interface MailConnector {
  id: string;
  agentId: string;
  type: MailProvider;
  accountLabel: string | null;
  status: ConnectorStatus;
  createdAtMs: number;
  updatedAtMs: number;
  lastSyncedAtMs?: number | null;
  error?: string | null;
}

export interface MailMessage {
  id: string;
  from: string;
  subject: string;
  preview: string;
  receivedAt: string;
}

export interface CreateMailDraftInput {
  to: string[];
  subject: string;
  body: string;
}

export interface MailDraft extends CreateMailDraftInput {
  id: string;
  connectorId: string;
  state: 'pending' | 'sending' | 'sent' | 'rejected' | 'failed' | 'unknown';
  error: string | null;
  createdAtMs: number;
  resolvedAtMs: number | null;
}

export interface MailStatus {
  configured: boolean;
  connector: MailConnector | null;
}

export interface MailConnectResult {
  connector: MailConnector;
  consentUrl: string;
}

export interface CalendarConnector {
  id: string;
  agentId: string;
  type: 'gcalendar';
  accountLabel: string | null;
  calendarIds: string[];
  status: ConnectorStatus;
  createdAtMs: number;
  updatedAtMs: number;
}

export interface CalendarEventDraft {
  calendarId: string;
  eventId: string | null;
  title: string;
  start: string;
  end: string;
  location: string | null;
  description: string | null;
}

export interface CalendarWrite {
  id: string;
  connectorId: string;
  operation: 'create' | 'update' | 'delete';
  draft: CalendarEventDraft;
  summary: string;
  state: 'pending' | 'applied' | 'rejected' | 'failed';
  error: string | null;
  createdAtMs: number;
  resolvedAtMs: number | null;
}

export interface CalendarStatus {
  configured: boolean;
  connector: CalendarConnector | null;
}

export interface CalendarConnectResult {
  connector: CalendarConnector;
  consentUrl: string;
}

/** Owner-facing integrations. Approvals must only follow explicit owner review. */
export class ConnectorsClient {
  constructor(private readonly client: DaemonClient) {}

  oauthAppStatus(provider: OAuthAppProvider): Promise<OAuthAppStatus> {
    return this.client.requestJson(oauthAppPath(provider));
  }

  configureOauthApp(
    provider: OAuthAppProvider,
    input: ConfigureOAuthAppInput,
  ): Promise<OAuthAppStatus> {
    return this.client.requestJson(oauthAppPath(provider), {
      method: 'PUT',
      body: input,
    });
  }

  async removeOauthApp(provider: OAuthAppProvider): Promise<void> {
    await this.client.requestJson(oauthAppPath(provider), {
      method: 'DELETE',
    });
  }

  mailStatus(agentId: string, provider: MailProvider): Promise<MailStatus> {
    return this.client.requestJson(mailPath(agentId, provider));
  }

  connectMail(
    agentId: string,
    provider: MailProvider,
  ): Promise<MailConnectResult> {
    return this.client.requestJson(mailPath(agentId, provider), {
      method: 'POST',
    });
  }

  async disconnectMail(
    agentId: string,
    provider: MailProvider,
    connectorId: string,
  ): Promise<void> {
    await this.client.requestJson(mailPath(agentId, provider, connectorId), {
      method: 'DELETE',
    });
  }

  async mailMessages(
    agentId: string,
    provider: MailProvider,
    connectorId: string,
    options: { refresh?: boolean } = {},
  ): Promise<MailMessage[]> {
    const response = await this.client.requestJson<{ messages: MailMessage[] }>(
      `${mailPath(agentId, provider, connectorId)}/messages${options.refresh ? '?refresh=true' : ''}`,
    );
    return response.messages;
  }

  async mailDrafts(
    agentId: string,
    provider: MailProvider,
    connectorId: string,
  ): Promise<MailDraft[]> {
    const response = await this.client.requestJson<{ drafts: MailDraft[] }>(
      `${mailPath(agentId, provider, connectorId)}/drafts`,
    );
    return response.drafts;
  }

  /** Persist a local draft. This does not send mail or create a provider draft. */
  async createMailDraft(
    agentId: string,
    provider: MailProvider,
    connectorId: string,
    input: CreateMailDraftInput,
  ): Promise<MailDraft> {
    const response = await this.client.requestJson<{ draft: MailDraft }>(
      `${mailPath(agentId, provider, connectorId)}/drafts`,
      { method: 'POST', body: input },
    );
    return response.draft;
  }

  /** Send the immutable draft once after explicit owner approval. Never retry automatically. */
  async approveMailDraft(
    agentId: string,
    provider: MailProvider,
    connectorId: string,
    draftId: string,
  ): Promise<MailDraft> {
    const response = await this.client.requestJson<{ draft: MailDraft }>(
      `${mailPath(agentId, provider, connectorId)}/drafts/${encodeURIComponent(draftId)}/approve`,
      { method: 'POST' },
    );
    return response.draft;
  }

  async rejectMailDraft(
    agentId: string,
    provider: MailProvider,
    connectorId: string,
    draftId: string,
  ): Promise<MailDraft> {
    const response = await this.client.requestJson<{ draft: MailDraft }>(
      `${mailPath(agentId, provider, connectorId)}/drafts/${encodeURIComponent(draftId)}/reject`,
      { method: 'POST' },
    );
    return response.draft;
  }

  calendarStatus(agentId: string): Promise<CalendarStatus> {
    return this.client.requestJson(calendarPath(agentId));
  }

  connectCalendar(agentId: string): Promise<CalendarConnectResult> {
    return this.client.requestJson(calendarPath(agentId), { method: 'POST' });
  }

  async disconnectCalendar(
    agentId: string,
    connectorId: string,
  ): Promise<void> {
    await this.client.requestJson(calendarPath(agentId, connectorId), {
      method: 'DELETE',
    });
  }

  async calendarWrites(
    agentId: string,
    connectorId: string,
  ): Promise<CalendarWrite[]> {
    const response = await this.client.requestJson<{ writes: CalendarWrite[] }>(
      `${calendarPath(agentId, connectorId)}/writes`,
    );
    return response.writes;
  }

  async approveCalendarWrite(
    agentId: string,
    connectorId: string,
    writeId: string,
  ): Promise<CalendarWrite> {
    const response = await this.client.requestJson<{ write: CalendarWrite }>(
      `${calendarPath(agentId, connectorId)}/writes/${encodeURIComponent(writeId)}/approve`,
      { method: 'POST' },
    );
    return response.write;
  }

  async rejectCalendarWrite(
    agentId: string,
    connectorId: string,
    writeId: string,
  ): Promise<CalendarWrite> {
    const response = await this.client.requestJson<{ write: CalendarWrite }>(
      `${calendarPath(agentId, connectorId)}/writes/${encodeURIComponent(writeId)}/reject`,
      { method: 'POST' },
    );
    return response.write;
  }
}

function oauthAppPath(provider: OAuthAppProvider): string {
  return `/api/connectors/oauth-apps/${encodeURIComponent(provider)}`;
}

function mailPath(
  agentId: string,
  provider: MailProvider,
  connectorId?: string,
): string {
  return connectorPath(agentId, [
    'mail',
    provider,
    ...(connectorId === undefined ? [] : [connectorId]),
  ]);
}

function calendarPath(agentId: string, connectorId?: string): string {
  return connectorPath(agentId, [
    'gcalendar',
    ...(connectorId === undefined ? [] : [connectorId]),
  ]);
}

function connectorPath(agentId: string, segments: string[]): string {
  return `/api/agents/${encodeURIComponent(agentId)}/connectors/${segments.map(encodeURIComponent).join('/')}`;
}
