import { useEffect, useRef, useState, type ReactNode } from 'react';
import {
  createDaemonClient,
  type MailProvider,
  type MailDraft,
  type OAuthAppProvider,
} from '@animaOS-SWARM/sdk';
import { primaryBtnCls } from './ui-bits';

const client = createDaemonClient({ baseUrl: '' }).connectors;
const button =
  'rounded-lg border border-line px-3 py-2 text-xs text-ink-2 hover:bg-white/[0.04] disabled:cursor-not-allowed disabled:opacity-50';

// Each service is keyed by owner. A generation also invalidates reads begun before a mutation.
function useService<T>(load: (explicit?: boolean) => Promise<T>) {
  const loader = useRef(load);
  loader.current = load;
  const alive = useRef(false),
    generation = useRef(0),
    locked = useRef(false);
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [loading, setLoading] = useState(true);
  async function refresh(clearError = true, explicit = false) {
    if (locked.current) return;
    const current = ++generation.current;
    setLoading(true);
    try {
      const next = await loader.current(explicit);
      if (alive.current && current === generation.current) {
        setData(next);
        if (clearError) setError(null);
      }
    } catch {
      if (alive.current && current === generation.current)
        setError('Could not refresh this connection. Try again.');
    } finally {
      if (alive.current && current === generation.current) setLoading(false);
    }
  }
  useEffect(() => {
    alive.current = true;
    void refresh();
    const timer = window.setInterval(() => {
      if (document.visibilityState !== 'hidden') void refresh();
    }, 15000);
    return () => {
      alive.current = false;
      generation.current++;
      window.clearInterval(timer);
    };
  }, []);
  async function run<R>(
    action: () => Promise<R>,
    onSuccess?: (result: R) => void,
  ) {
    if (locked.current || !alive.current) return false;
    locked.current = true;
    generation.current++;
    setBusy(true);
    setError(null);
    let success = false;
    try {
      const result = await action();
      success = true;
      if (alive.current) onSuccess?.(result);
    } catch (cause) {
      if (alive.current)
        setError(
          cause instanceof Error &&
            cause.message ===
              'The provider returned an invalid authorization link.'
            ? cause.message
            : 'This action could not be completed. Refresh the connection before trying again.',
        );
    } finally {
      locked.current = false;
      if (alive.current) {
        await refresh(false);
        if (alive.current) setBusy(false);
      }
    }
    return success && alive.current;
  }
  return { data, error, busy, loading, refresh, run };
}

function authorizationUrl(value: string, provider: 'google' | 'outlook') {
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw new Error('The provider returned an invalid authorization link.');
  }
  const origin =
    provider === 'google'
      ? 'https://accounts.google.com'
      : 'https://login.microsoftonline.com';
  if (url.origin !== origin || url.username || url.password)
    throw new Error('The provider returned an invalid authorization link.');
  return url.href;
}
function Card({
  title,
  description,
  children,
}: {
  title: string;
  description: string;
  children: ReactNode;
}) {
  return (
    <section
      aria-label={title}
      className="min-w-0 space-y-4 rounded-2xl border border-line bg-panel/60 p-4 sm:p-6"
    >
      <div>
        <h2 className="text-base font-semibold text-ink">{title}</h2>
        <p className="mt-1 text-xs leading-relaxed text-ink-3">{description}</p>
      </div>
      {children}
    </section>
  );
}
function ConnectionControls({
  name,
  configured,
  connected,
  status,
  account,
  busy,
  loading,
  error,
  consent,
  provider,
  onConnect,
  onDisconnect,
  onRefresh,
}: {
  name: string;
  configured: boolean;
  connected: boolean;
  status: string;
  account?: string | null;
  busy: boolean;
  loading: boolean;
  error: string | null;
  consent: string | null;
  provider: 'google' | 'outlook';
  onConnect: () => void;
  onDisconnect: () => void;
  onRefresh: () => void;
}) {
  return (
    <>
      <div className="space-y-1 text-xs">
        <p role="status" className="font-medium text-accent">
          {loading && !status ? 'Loading connection…' : status}
        </p>
        {account && <p className="break-all text-ink-2">{account}</p>}
      </div>
      {status === 'Setup required' && !loading && (
        <p className="text-xs leading-relaxed text-ink-3">
          OAuth is not configured on this daemon. Ask the workspace owner to
          configure {provider === 'google' ? 'Google' : 'Microsoft'}{' '}
          credentials, then refresh.
        </p>
      )}
      <div className="flex flex-wrap gap-2">
        <button
          className={primaryBtnCls}
          disabled={busy || loading || !configured}
          onClick={onConnect}
        >
          {connected ? `Reconnect ${name}` : `Connect ${name}`}
        </button>
        {connected && (
          <button className={button} disabled={busy} onClick={onDisconnect}>
            Disconnect {name}
          </button>
        )}
        <button
          className={button}
          disabled={busy || loading}
          onClick={onRefresh}
          aria-label={`Refresh ${name}`}
        >
          {loading ? 'Refreshing…' : 'Refresh'}
        </button>
      </div>
      {consent && (
        <a
          href={consent}
          target="_blank"
          rel="noopener noreferrer"
          className="inline-flex text-sm text-accent underline"
        >
          Continue to {provider === 'google' ? 'Google' : 'Microsoft'}
        </a>
      )}
      {error && (
        <p role="alert" className="text-xs text-danger">
          {error}
        </p>
      )}
    </>
  );
}
function MailCard({
  agentId,
  provider,
}: {
  agentId: string;
  provider: MailProvider;
}) {
  const name = provider === 'gmail' ? 'Gmail' : 'Outlook';
  const [consent, setConsent] = useState<string | null>(null);
  const [to, setTo] = useState(''),
    [subject, setSubject] = useState(''),
    [body, setBody] = useState('');
  const resource = useService(async (explicit = false) => {
    const status = await client.mailStatus(agentId, provider);
    const id = status.connector?.id;
    const [messages, drafts] = await Promise.allSettled([
      id && status.connector?.status === 'active'
        ? client.mailMessages(agentId, provider, id, { refresh: explicit })
        : Promise.resolve([]),
      id ? client.mailDrafts(agentId, provider, id) : Promise.resolve([]),
    ]);
    return {
      ...status,
      messages: messages.status === 'fulfilled' ? messages.value : [],
      drafts: drafts.status === 'fulfilled' ? drafts.value : [],
      inboxError: messages.status === 'rejected',
      draftsError: drafts.status === 'rejected',
    };
  });
  const connection = resource.data?.connector;
  useEffect(() => {
    if (connection?.status === 'active') setConsent(null);
  }, [connection?.status]);
  const status = !resource.data
    ? resource.error
      ? 'Status unavailable'
      : ''
    : !resource.data.configured
      ? 'Setup required'
      : !connection
        ? 'Not connected'
        : connection.status === 'active'
          ? 'Connected'
          : connection.status === 'reauthRequired'
            ? 'Reconnect required'
            : 'Awaiting authorization';
  return (
    <Card
      title={name}
      description="Read recent mail and prepare messages for your review."
    >
      <ConnectionControls
        name={name}
        configured={resource.data?.configured ?? false}
        connected={!!connection}
        status={status}
        account={connection?.accountLabel}
        {...resource}
        consent={consent}
        provider={provider === 'gmail' ? 'google' : 'outlook'}
        onConnect={() => {
          setConsent(null);
          void resource.run(async () => {
            const result = await client.connectMail(agentId, provider);
            const url = authorizationUrl(
              result.consentUrl,
              provider === 'gmail' ? 'google' : 'outlook',
            );
            return url;
          }, setConsent);
        }}
        onDisconnect={() => {
          setConsent(null);
          void resource.run(() =>
            client.disconnectMail(agentId, provider, connection!.id),
          );
        }}
        onRefresh={() => void resource.refresh(true, true)}
      />
      {connection?.error && (
        <p role="alert" className="text-xs text-danger">
          {connection.error}
        </p>
      )}
      <p className="text-xs leading-relaxed text-ink-3">
        Drafts stay in animaOS until you approve Send. Connecting an account
        never sends mail.
      </p>
      {connection && (
        <>
          <div className="space-y-3 border-t border-line pt-4">
            <h3 className="text-sm font-medium text-ink">Recent inbox</h3>
            {resource.data?.inboxError && (
              <p role="alert" className="text-xs text-danger">
                Could not load recent inbox. Refresh to try again.
              </p>
            )}
            {!resource.data?.inboxError &&
              resource.data?.messages.length === 0 && (
                <p className="text-xs text-ink-3">
                  {connection.status === 'active'
                    ? 'No recent messages.'
                    : 'Reconnect to read recent messages.'}
                </p>
              )}
            {resource.data?.messages.map((message) => (
              <article
                key={message.id}
                className="min-w-0 rounded-xl border border-line p-3 text-xs"
              >
                <p className="break-words font-medium text-ink">
                  {message.subject || '(No subject)'}
                </p>
                <p className="mt-1 break-all text-ink-3">
                  {message.from} · {message.receivedAt}
                </p>
                <p className="mt-2 whitespace-pre-wrap break-words text-ink-2">
                  {message.preview}
                </p>
              </article>
            ))}
          </div>
          <form
            className="space-y-3 border-t border-line pt-4"
            onSubmit={(event) => {
              event.preventDefault();
              void resource
                .run(() =>
                  client.createMailDraft(agentId, provider, connection.id, {
                    to: to
                      .split(',')
                      .map((value) => value.trim())
                      .filter(Boolean),
                    subject,
                    body,
                  }),
                )
                .then((ok) => {
                  if (ok) {
                    setTo('');
                    setSubject('');
                    setBody('');
                  }
                });
            }}
          >
            <h3 className="text-sm font-medium text-ink">New local draft</h3>
            <label className="block text-xs text-ink-2">
              To (comma-separated)
              <input
                className="field mt-1 w-full"
                aria-label={`${name} draft recipients`}
                value={to}
                onChange={(e) => setTo(e.target.value)}
                required
                disabled={resource.busy}
              />
            </label>
            <label className="block text-xs text-ink-2">
              Subject
              <input
                className="field mt-1 w-full"
                aria-label={`${name} draft subject`}
                value={subject}
                onChange={(e) => setSubject(e.target.value)}
                maxLength={998}
                disabled={resource.busy}
              />
            </label>
            <label className="block text-xs text-ink-2">
              Message
              <textarea
                className="field mt-1 min-h-28 w-full resize-y"
                aria-label={`${name} draft message`}
                value={body}
                onChange={(e) => setBody(e.target.value)}
                required
                maxLength={100000}
                disabled={resource.busy}
              />
            </label>
            <button
              className={button}
              type="submit"
              disabled={resource.busy || !to.trim() || !body.trim()}
            >
              Save local draft
            </button>
          </form>
          <div className="space-y-3 border-t border-line pt-4">
            <h3 className="text-sm font-medium text-ink">Saved drafts</h3>
            {resource.data?.draftsError && (
              <p role="alert" className="text-xs text-danger">
                Could not load saved drafts. Refresh to try again.
              </p>
            )}
            {!resource.data?.draftsError && !resource.data?.drafts.length && (
              <p className="text-xs text-ink-3">No saved drafts yet.</p>
            )}
            {resource.data?.drafts.map((draft) => (
              <DraftReview
                key={draft.id}
                draft={draft}
                busy={resource.busy}
                canSend={connection.status === 'active'}
                approve={() =>
                  void resource.run(() =>
                    client.approveMailDraft(
                      agentId,
                      provider,
                      connection.id,
                      draft.id,
                    ),
                  )
                }
                reject={() =>
                  void resource.run(() =>
                    client.rejectMailDraft(
                      agentId,
                      provider,
                      connection.id,
                      draft.id,
                    ),
                  )
                }
              />
            ))}
          </div>
        </>
      )}
    </Card>
  );
}
function DraftReview({
  draft,
  busy,
  canSend,
  approve,
  reject,
}: {
  draft: MailDraft;
  busy: boolean;
  canSend: boolean;
  approve: () => void;
  reject: () => void;
}) {
  return (
    <article className="min-w-0 space-y-2 rounded-xl border border-line p-3 text-xs">
      <p className="break-words font-medium text-ink">
        {draft.subject || '(No subject)'}
      </p>
      <p className="break-all text-ink-3">To: {draft.to.join(', ')}</p>
      <p className="max-h-64 overflow-y-auto whitespace-pre-wrap break-words text-ink-2">
        {draft.body}
      </p>
      <p className="capitalize text-ink-3">{draft.state}</p>
      {draft.error && (
        <p role="alert" className="text-danger">
          {draft.error}
        </p>
      )}
      {draft.state === 'unknown' && (
        <p className="text-danger">
          Delivery is uncertain. Check your sent mail before creating another
          draft.
        </p>
      )}
      {draft.state === 'pending' && (
        <div className="flex flex-wrap gap-2">
          <button
            className={primaryBtnCls}
            disabled={busy || !canSend}
            onClick={approve}
          >
            Send saved draft
          </button>
          <button className={button} disabled={busy} onClick={reject}>
            Reject draft
          </button>
        </div>
      )}
    </article>
  );
}

function OAuthAppCard({
  provider,
  onChanged,
}: {
  provider: OAuthAppProvider;
  onChanged: () => void;
}) {
  const name = provider === 'google' ? 'Google' : 'Microsoft';
  const [clientId, setClientId] = useState('');
  const [clientSecret, setClientSecret] = useState('');
  const [tenant, setTenant] = useState('common');
  const [validationError, setValidationError] = useState<string | null>(null);
  const resource = useService(() => client.oauthAppStatus(provider));
  const status = resource.data;
  const environmentManaged = status?.source === 'environment';

  useEffect(() => {
    if (provider === 'microsoft' && status?.tenant) setTenant(status.tenant);
  }, [provider, status?.tenant]);

  const submit = () => {
    setValidationError(null);
    const submittedClientId = clientId.trim();
    const submittedClientSecret = clientSecret.trim();
    const submittedTenant = tenant.trim();
    if (!submittedClientId || !submittedClientSecret) {
      setValidationError('Client ID and client secret are required.');
      return;
    }
    setClientSecret('');
    void resource
      .run(() =>
        client.configureOauthApp(provider, {
          clientId: submittedClientId,
          clientSecret: submittedClientSecret,
          ...(provider === 'microsoft' ? { tenant: submittedTenant } : {}),
        }),
      )
      .then((ok) => {
        if (!ok) return;
        setClientId('');
        setClientSecret('');
        onChanged();
      });
  };

  return (
    <section
      aria-label={`${name} OAuth app`}
      className="min-w-0 space-y-3 rounded-xl border border-line p-4"
    >
      <div>
        <h3 className="text-sm font-semibold text-ink">{name} OAuth app</h3>
        <p className="mt-1 text-xs leading-relaxed text-ink-3">
          {provider === 'google'
            ? 'One OAuth app enables Gmail and Google Calendar.'
            : 'Enables Outlook.'}
        </p>
      </div>
      <p role="status" className="text-xs font-medium text-accent">
        {resource.loading && !status
          ? 'Loading setup…'
          : status?.configured
            ? 'Configured'
            : resource.error
              ? 'Status unavailable'
              : 'Setup required'}
      </p>
      {status?.source && (
        <p className="text-xs text-ink-3">
          Source: {status.source === 'vault' ? 'Daemon vault' : 'Environment'}
        </p>
      )}
      {status?.clientIdHint && (
        <p className="break-all text-xs text-ink-2">{status.clientIdHint}</p>
      )}
      <div className="space-y-1 text-xs text-ink-2">
        <p>Redirect {status?.redirectUris.length === 1 ? 'URI' : 'URIs'}</p>
        {status?.redirectUris.map((uri) => (
          <code
            key={uri}
            className="block overflow-x-auto rounded-lg bg-black/20 px-2 py-1.5 text-ink-3"
          >
            {uri}
          </code>
        ))}
      </div>
      {environmentManaged && (
        <div className="space-y-1 text-xs leading-relaxed text-ink-3">
          <p>Managed through the daemon environment.</p>
          <p>These credentials cannot be replaced or removed in the UI.</p>
        </div>
      )}
      <form
        className="grid min-w-0 gap-3 sm:grid-cols-2"
        onSubmit={(event) => {
          event.preventDefault();
          submit();
        }}
      >
        <label className="block min-w-0 text-xs text-ink-2">
          Client ID
          <input
            className="field mt-1 w-full"
            aria-label={`${name} client ID`}
            value={clientId}
            onChange={(event) => setClientId(event.target.value)}
            disabled={resource.busy || environmentManaged}
          />
        </label>
        <label className="block min-w-0 text-xs text-ink-2">
          Client secret
          <input
            className="field mt-1 w-full"
            aria-label={`${name} client secret`}
            type="password"
            autoComplete="new-password"
            value={clientSecret}
            onChange={(event) => setClientSecret(event.target.value)}
            disabled={resource.busy || environmentManaged}
          />
        </label>
        {provider === 'microsoft' && (
          <label className="block min-w-0 text-xs text-ink-2 sm:col-span-2">
            Tenant
            <input
              className="field mt-1 w-full"
              aria-label="Microsoft tenant"
              value={tenant}
              onChange={(event) => setTenant(event.target.value)}
              disabled={resource.busy || environmentManaged}
            />
          </label>
        )}
        <div className="flex flex-wrap gap-2 sm:col-span-2">
          <button
            className={primaryBtnCls}
            type="submit"
            disabled={resource.busy || resource.loading || environmentManaged}
          >
            Save {name} OAuth app
          </button>
          {status?.source === 'vault' && (
            <button
              className={button}
              type="button"
              disabled={resource.busy}
              onClick={() => {
                setValidationError(null);
                setClientSecret('');
                void resource
                  .run(() => client.removeOauthApp(provider))
                  .then((ok) => {
                    if (ok) onChanged();
                  });
              }}
            >
              Remove {name} OAuth app
            </button>
          )}
        </div>
      </form>
      {(validationError || resource.error) && (
        <p role="alert" className="text-xs text-danger">
          {validationError || resource.error}
        </p>
      )}
    </section>
  );
}

function CalendarCard({ agentId }: { agentId: string }) {
  const [consent, setConsent] = useState<string | null>(null);
  const resource = useService(async () => {
    const status = await client.calendarStatus(agentId);
    const [writes] = await Promise.allSettled([
      status.connector
        ? client.calendarWrites(agentId, status.connector.id)
        : Promise.resolve([]),
    ]);
    return {
      ...status,
      writes: writes.status === 'fulfilled' ? writes.value : [],
      writesError: writes.status === 'rejected',
    };
  });
  const connection = resource.data?.connector;
  const status = !resource.data
    ? resource.error
      ? 'Status unavailable'
      : ''
    : !resource.data.configured
      ? 'Setup required'
      : !connection
        ? 'Not connected'
        : connection.status === 'active'
          ? 'Connected'
          : connection.status === 'reauthRequired'
            ? 'Reconnect required'
            : 'Awaiting authorization';
  useEffect(() => {
    if (connection?.status === 'active') setConsent(null);
  }, [connection?.status]);
  return (
    <Card
      title="Google Calendar"
      description="See your schedule. Review calendar changes before they are applied."
    >
      <ConnectionControls
        name="Google Calendar"
        configured={resource.data?.configured ?? false}
        connected={!!connection}
        status={status}
        account={connection?.accountLabel}
        {...resource}
        consent={consent}
        provider="google"
        onConnect={() => {
          setConsent(null);
          void resource.run(async () => {
            const result = await client.connectCalendar(agentId);
            return authorizationUrl(result.consentUrl, 'google');
          }, setConsent);
        }}
        onDisconnect={() => {
          setConsent(null);
          void resource.run(() =>
            client.disconnectCalendar(agentId, connection!.id),
          );
        }}
        onRefresh={() => void resource.refresh()}
      />
      {connection && (
        <div className="space-y-3 border-t border-line pt-4">
          <h3 className="text-sm font-medium text-ink">Calendar approvals</h3>
          {resource.data?.writesError && (
            <p role="alert" className="text-xs text-danger">
              Could not load calendar approvals. Refresh to try again.
            </p>
          )}
          {!resource.data?.writesError && !resource.data?.writes.length && (
            <p className="text-xs text-ink-3">
              No calendar changes awaiting review.
            </p>
          )}
          {resource.data?.writes.map((write) => (
            <article
              key={write.id}
              className="space-y-2 rounded-xl border border-line p-3 text-xs"
            >
              <p className="break-words font-medium text-ink">
                {write.summary}
              </p>
              <p className="text-ink-3">
                {write.operation} · {write.state}
              </p>
              <p className="break-words text-ink-2">{write.draft.title}</p>
              <p className="break-words text-ink-3">
                {write.draft.start} → {write.draft.end}
              </p>
              <p className="break-all text-ink-3">
                Calendar: {write.draft.calendarId}
              </p>
              {write.draft.location && (
                <p className="break-words text-ink-2">{write.draft.location}</p>
              )}
              {write.draft.description && (
                <p className="whitespace-pre-wrap break-words text-ink-2">
                  {write.draft.description}
                </p>
              )}
              {write.error && (
                <p role="alert" className="text-danger">
                  {write.error}
                </p>
              )}
              {write.state === 'pending' && (
                <div className="flex flex-wrap gap-2">
                  <button
                    className={primaryBtnCls}
                    disabled={resource.busy || connection.status !== 'active'}
                    onClick={() =>
                      void resource.run(() =>
                        client.approveCalendarWrite(
                          agentId,
                          connection.id,
                          write.id,
                        ),
                      )
                    }
                  >
                    Approve calendar change
                  </button>
                  <button
                    className={button}
                    disabled={resource.busy}
                    onClick={() =>
                      void resource.run(() =>
                        client.rejectCalendarWrite(
                          agentId,
                          connection.id,
                          write.id,
                        ),
                      )
                    }
                  >
                    Reject change
                  </button>
                </div>
              )}
            </article>
          ))}
        </div>
      )}
    </Card>
  );
}
export function ConnectorsView({
  agentId,
  telegram,
}: {
  agentId: string;
  telegram: ReactNode;
}) {
  const [oauthGeneration, setOauthGeneration] = useState({
    google: 0,
    microsoft: 0,
  });
  const oauthChanged = (provider: OAuthAppProvider) =>
    setOauthGeneration((current) => ({
      ...current,
      [provider]: current[provider] + 1,
    }));

  return (
    <section
      aria-label="Connectors"
      className="h-full min-w-0 overflow-y-auto p-4 pb-8 sm:p-6 lg:p-8"
    >
      <div className="mx-auto max-w-6xl space-y-6">
        <header>
          <h1 className="text-xl font-semibold text-ink">Connectors</h1>
          <p className="mt-2 max-w-2xl text-sm leading-relaxed text-ink-3">
            Bring your conversations, calendar, and inbox into your workspace.
            Connections belong to your main agent.
          </p>
        </header>
        <section
          aria-labelledby="oauth-app-setup-heading"
          className="space-y-4 rounded-2xl border border-line bg-panel/60 p-4 sm:p-6"
        >
          <div>
            <h2
              id="oauth-app-setup-heading"
              className="text-base font-semibold text-ink"
            >
              OAuth app setup
            </h2>
            <p className="mt-1 text-xs leading-relaxed text-ink-3">
              Add the daemon credentials used to start account connections.
            </p>
          </div>
          <div className="grid min-w-0 items-start gap-4 lg:grid-cols-2">
            <OAuthAppCard
              provider="google"
              onChanged={() => oauthChanged('google')}
            />
            <OAuthAppCard
              provider="microsoft"
              onChanged={() => oauthChanged('microsoft')}
            />
          </div>
        </section>
        <div className="grid min-w-0 items-start gap-4 xl:grid-cols-2">
          <div className="min-w-0 rounded-2xl border border-line bg-panel/60 p-4 sm:p-6">
            {telegram}
          </div>
          <CalendarCard
            key={`${agentId}:calendar:${oauthGeneration.google}`}
            agentId={agentId}
          />
          <MailCard
            key={`${agentId}:gmail:${oauthGeneration.google}`}
            agentId={agentId}
            provider="gmail"
          />
          <MailCard
            key={`${agentId}:outlook:${oauthGeneration.microsoft}`}
            agentId={agentId}
            provider="outlook"
          />
        </div>
      </div>
    </section>
  );
}
