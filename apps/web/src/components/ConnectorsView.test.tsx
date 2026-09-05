import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import { ConnectorsView } from './ConnectorsView';
const api = vi.hoisted(() => ({
  calendarStatus: vi.fn(),
  mailStatus: vi.fn(),
  connectMail: vi.fn(),
  disconnectMail: vi.fn(),
  mailMessages: vi.fn(),
  mailDrafts: vi.fn(),
  createMailDraft: vi.fn(),
  approveMailDraft: vi.fn(),
  rejectMailDraft: vi.fn(),
  calendarWrites: vi.fn(),
  approveCalendarWrite: vi.fn(),
  rejectCalendarWrite: vi.fn(),
  oauthAppStatus: vi.fn(),
  configureOauthApp: vi.fn(),
  removeOauthApp: vi.fn(),
}));
vi.mock('@animaOS-SWARM/sdk', () => ({
  createDaemonClient: () => ({ connectors: api }),
}));
const connector = {
  id: 'gmail-1',
  agentId: 'main',
  type: 'gmail',
  accountLabel: 'owner@example.com',
  status: 'active',
};
const draft = {
  id: 'draft-1',
  connectorId: 'gmail-1',
  to: ['friend@example.com'],
  subject: 'Saved subject',
  body: 'Saved message',
  state: 'pending',
  error: null,
};
beforeEach(() => {
  vi.resetAllMocks();
  api.oauthAppStatus.mockImplementation((provider) =>
    Promise.resolve({
      provider,
      configured: false,
      source: null,
      clientIdHint: null,
      redirectUris:
        provider === 'google'
          ? ['http://127.0.0.1:8080/api/connectors/google/callback']
          : ['http://127.0.0.1:8080/api/connectors/microsoft/callback'],
      tenant: provider === 'microsoft' ? 'common' : null,
    }),
  );
  api.calendarStatus.mockResolvedValue({ configured: false, connector: null });
  api.mailStatus.mockResolvedValue({ configured: false, connector: null });
  api.mailMessages.mockResolvedValue([]);
  api.mailDrafts.mockResolvedValue([]);
});
it('shows all four services with honest OAuth setup state', async () => {
  render(
    <ConnectorsView agentId="main" telegram={<div>Telegram management</div>} />,
  );
  expect(screen.getByText('Telegram management')).toBeVisible();
  for (const name of ['Google Calendar', 'Gmail', 'Outlook'])
    expect(screen.getByRole('heading', { name })).toBeVisible();
  await waitFor(() =>
    expect(screen.getAllByText('Setup required')).toHaveLength(5),
  );
  expect(screen.getByRole('button', { name: 'Connect Gmail' })).toBeDisabled();
  expect(api.approveMailDraft).not.toHaveBeenCalled();
});
it('sends the reviewed saved draft only after explicit Send', async () => {
  api.mailStatus.mockImplementation((_id, provider) =>
    Promise.resolve({
      configured: true,
      connector: provider === 'gmail' ? connector : null,
    }),
  );
  api.mailDrafts.mockResolvedValue([draft]);
  api.approveMailDraft.mockResolvedValue({ ...draft, state: 'sent' });
  render(<ConnectorsView agentId="main" telegram={null} />);
  expect(await screen.findByText('Saved message')).toBeVisible();
  expect(api.approveMailDraft).not.toHaveBeenCalled();
  await userEvent.click(
    screen.getByRole('button', { name: 'Send saved draft' }),
  );
  expect(api.approveMailDraft).toHaveBeenCalledWith(
    'main',
    'gmail',
    'gmail-1',
    'draft-1',
  );
});
it('rejects an OAuth link from an untrusted origin', async () => {
  api.mailStatus.mockResolvedValue({ configured: true, connector: null });
  api.connectMail.mockResolvedValue({
    connector,
    consentUrl: 'https://accounts.google.com.attacker.test/consent',
  });
  render(<ConnectorsView agentId="main" telegram={null} />);
  await waitFor(() =>
    expect(screen.getByRole('button', { name: 'Connect Gmail' })).toBeEnabled(),
  );
  await userEvent.click(screen.getByRole('button', { name: 'Connect Gmail' }));
  expect(
    await screen.findByText(
      'The provider returned an invalid authorization link.',
    ),
  ).toBeVisible();
  expect(
    screen.queryByRole('link', { name: 'Continue to Google' }),
  ).not.toBeInTheDocument();
});
it('ignores an old account response after the main agent changes', async () => {
  let resolve!: (value: unknown) => void;
  api.mailStatus.mockImplementation((id) =>
    id === 'old'
      ? new Promise((r) => {
          resolve = r;
        })
      : Promise.resolve({ configured: false, connector: null }),
  );
  const view = render(<ConnectorsView agentId="old" telegram={null} />);
  view.rerender(<ConnectorsView agentId="new" telegram={null} />);
  resolve({ configured: true, connector });
  await waitFor(() =>
    expect(screen.getAllByText('Setup required')).toHaveLength(5),
  );
  expect(screen.queryByText('owner@example.com')).not.toBeInTheDocument();
});

it('offers an official consent link without sending mail and clears it on disconnect', async () => {
  const pairing = { ...connector, status: 'pairing' };
  api.mailStatus.mockImplementation((_id, provider) =>
    Promise.resolve({
      configured: true,
      connector: provider === 'gmail' ? pairing : null,
    }),
  );
  api.connectMail.mockResolvedValue({
    connector: pairing,
    consentUrl: 'https://accounts.google.com/o/oauth2/v2/auth?state=abc',
  });
  api.disconnectMail.mockResolvedValue(undefined);
  render(<ConnectorsView agentId="main" telegram={null} />);
  await waitFor(() =>
    expect(
      screen.getByRole('button', { name: 'Reconnect Gmail' }),
    ).toBeEnabled(),
  );
  await userEvent.click(
    screen.getByRole('button', { name: 'Reconnect Gmail' }),
  );
  const link = await screen.findByRole('link', { name: 'Continue to Google' });
  expect(link).toHaveAttribute('rel', 'noopener noreferrer');
  expect(link).toHaveAttribute(
    'href',
    'https://accounts.google.com/o/oauth2/v2/auth?state=abc',
  );
  expect(api.approveMailDraft).not.toHaveBeenCalled();
  await userEvent.click(
    screen.getByRole('button', { name: 'Disconnect Gmail' }),
  );
  expect(api.disconnectMail).toHaveBeenCalledWith('main', 'gmail', 'gmail-1');
  expect(
    screen.queryByRole('link', { name: 'Continue to Google' }),
  ).not.toBeInTheDocument();
});
it('saves a local draft without sending and rejects the saved draft by ID', async () => {
  api.mailStatus.mockImplementation((_id, provider) =>
    Promise.resolve({
      configured: true,
      connector: provider === 'gmail' ? connector : null,
    }),
  );
  api.createMailDraft.mockResolvedValue(draft);
  api.mailDrafts.mockResolvedValue([draft]);
  const user = userEvent.setup();
  render(<ConnectorsView agentId="main" telegram={null} />);
  await user.type(
    await screen.findByRole('textbox', { name: 'Gmail draft recipients' }),
    'friend@example.com, other@example.com',
  );
  await user.type(
    screen.getByRole('textbox', { name: 'Gmail draft subject' }),
    'New subject',
  );
  await user.type(
    screen.getByRole('textbox', { name: 'Gmail draft message' }),
    'New unsent message',
  );
  await user.click(screen.getByRole('button', { name: 'Save local draft' }));
  expect(api.createMailDraft).toHaveBeenCalledWith('main', 'gmail', 'gmail-1', {
    to: ['friend@example.com', 'other@example.com'],
    subject: 'New subject',
    body: 'New unsent message',
  });
  expect(api.approveMailDraft).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: 'Reject draft' }));
  expect(api.rejectMailDraft).toHaveBeenCalledWith(
    'main',
    'gmail',
    'gmail-1',
    'draft-1',
  );
});
it('shows immutable calendar write details and requires explicit approval', async () => {
  api.calendarStatus.mockResolvedValue({
    configured: true,
    connector: {
      id: 'calendar-1',
      status: 'active',
      accountLabel: 'owner@example.com',
    },
  });
  api.calendarWrites.mockResolvedValue([
    {
      id: 'write-1',
      summary: 'Create lunch',
      state: 'pending',
      operation: 'create',
      draft: {
        title: 'Lunch',
        calendarId: 'primary',
        start: '2026-09-06T12:00:00Z',
        end: '2026-09-06T13:00:00Z',
        location: 'Cafe',
        description: 'Discuss roadmap',
      },
    },
  ]);
  render(<ConnectorsView agentId="main" telegram={null} />);
  expect(await screen.findByText('Discuss roadmap')).toBeVisible();
  expect(api.approveCalendarWrite).not.toHaveBeenCalled();
  await userEvent.click(
    screen.getByRole('button', { name: 'Approve calendar change' }),
  );
  expect(api.approveCalendarWrite).toHaveBeenCalledWith(
    'main',
    'calendar-1',
    'write-1',
  );
});
it('retains account controls when inbox loading fails', async () => {
  api.mailStatus.mockImplementation((_id, provider) =>
    Promise.resolve({
      configured: true,
      connector: provider === 'gmail' ? connector : null,
    }),
  );
  api.mailMessages.mockRejectedValue(new Error('private provider detail'));
  render(<ConnectorsView agentId="main" telegram={null} />);
  expect(
    await screen.findByRole('button', { name: 'Disconnect Gmail' }),
  ).toBeVisible();
  expect(
    screen.getByText('Could not load recent inbox. Refresh to try again.'),
  ).toBeVisible();
  expect(screen.queryByText('private provider detail')).not.toBeInTheDocument();
});

it('retains calendar controls when approvals cannot be loaded', async () => {
  api.calendarStatus.mockResolvedValue({
    configured: true,
    connector: { id: 'cal-1', status: 'active' },
  });
  api.calendarWrites.mockRejectedValue(new Error('private detail'));
  render(<ConnectorsView agentId="main" telegram={null} />);
  expect(
    await screen.findByRole('button', { name: 'Disconnect Google Calendar' }),
  ).toBeVisible();
  expect(
    screen.getByText(
      'Could not load calendar approvals. Refresh to try again.',
    ),
  ).toBeVisible();
});
it('does not reveal old consent after changing agents during connection', async () => {
  let resolve!: (value: unknown) => void;
  api.mailStatus.mockResolvedValue({ configured: true, connector: null });
  api.connectMail.mockImplementation(
    () =>
      new Promise((r) => {
        resolve = r;
      }),
  );
  const view = render(<ConnectorsView agentId="old" telegram={null} />);
  await waitFor(() =>
    expect(screen.getByRole('button', { name: 'Connect Gmail' })).toBeEnabled(),
  );
  await userEvent.click(screen.getByRole('button', { name: 'Connect Gmail' }));
  view.rerender(<ConnectorsView agentId="new" telegram={null} />);
  resolve({
    connector,
    consentUrl: 'https://accounts.google.com/o/oauth2/v2/auth?state=old',
  });
  await waitFor(() =>
    expect(screen.getByRole('button', { name: 'Connect Gmail' })).toBeEnabled(),
  );
  expect(
    screen.queryByRole('link', { name: 'Continue to Google' }),
  ).not.toBeInTheDocument();
});

it('does not misreport a failed status request as missing OAuth setup', async () => {
  api.calendarStatus.mockRejectedValue(new Error('offline'));
  api.mailStatus.mockRejectedValue(new Error('offline'));
  render(<ConnectorsView agentId="main" telegram={null} />);
  await waitFor(() =>
    expect(screen.getAllByText('Status unavailable')).toHaveLength(3),
  );
  expect(screen.queryByText(/OAuth is not configured/)).not.toBeInTheDocument();
});

it('refreshes inbox from the provider only on explicit Refresh', async () => {
  api.mailStatus.mockImplementation((_id, provider) =>
    Promise.resolve({
      configured: true,
      connector: provider === 'gmail' ? connector : null,
    }),
  );
  render(<ConnectorsView agentId="main" telegram={null} />);
  await waitFor(() =>
    expect(screen.getByRole('button', { name: 'Refresh Gmail' })).toBeEnabled(),
  );
  expect(api.mailMessages).not.toHaveBeenCalledWith(
    'main',
    'gmail',
    'gmail-1',
    { refresh: true },
  );
  await userEvent.click(screen.getByRole('button', { name: 'Refresh Gmail' }));
  await waitFor(() =>
    expect(api.mailMessages).toHaveBeenCalledWith('main', 'gmail', 'gmail-1', {
      refresh: true,
    }),
  );
});

it('shows OAuth redirect URIs and accessible credential fields', async () => {
  render(<ConnectorsView agentId="main" telegram={null} />);

  expect(
    screen.getByRole('heading', { name: 'OAuth app setup' }),
  ).toBeVisible();
  expect(
    screen.getByText('One OAuth app enables Gmail and Google Calendar.'),
  ).toBeVisible();
  expect(screen.getByText('Enables Outlook.')).toBeVisible();
  expect(
    await screen.findByText(
      'http://127.0.0.1:8080/api/connectors/google/callback',
    ),
  ).toBeVisible();
  expect(
    screen.getByText('http://127.0.0.1:8080/api/connectors/microsoft/callback'),
  ).toBeVisible();
  expect(screen.getByLabelText('Google client secret')).toHaveAttribute(
    'type',
    'password',
  );
  expect(screen.getByLabelText('Google client secret')).toHaveAttribute(
    'autocomplete',
    'new-password',
  );
  expect(screen.getByLabelText('Microsoft tenant')).toHaveValue('common');
});

it('saves Google credentials without connecting and refreshes both Google services', async () => {
  let googleConfigured = false;
  api.oauthAppStatus.mockImplementation((provider) =>
    Promise.resolve({
      provider,
      configured: false,
      source: null,
      clientIdHint: null,
      redirectUris: [`https://daemon.test/${provider}/callback`],
      tenant: provider === 'microsoft' ? 'common' : null,
    }),
  );
  api.configureOauthApp.mockImplementation((provider, input) => {
    expect(provider).toBe('google');
    expect(input).toEqual({
      clientId: 'google-id',
      clientSecret: 'top-secret',
    });
    googleConfigured = true;
    return Promise.resolve({
      provider,
      configured: true,
      source: 'vault',
      clientIdHint: 'goo…-id',
      redirectUris: ['https://daemon.test/google/callback'],
      tenant: null,
    });
  });
  api.mailStatus.mockImplementation((_id, provider) => {
    return Promise.resolve({
      configured: provider === 'gmail' && googleConfigured,
      connector: null,
    });
  });
  api.calendarStatus.mockImplementation(() =>
    Promise.resolve({
      configured: googleConfigured,
      connector: null,
    }),
  );
  const user = userEvent.setup();
  render(<ConnectorsView agentId="main" telegram={null} />);
  await user.type(
    await screen.findByLabelText('Google client ID'),
    'google-id',
  );
  await user.type(screen.getByLabelText('Google client secret'), 'top-secret');
  await user.click(
    screen.getByRole('button', { name: 'Save Google OAuth app' }),
  );

  await waitFor(() =>
    expect(api.configureOauthApp).toHaveBeenCalledWith('google', {
      clientId: 'google-id',
      clientSecret: 'top-secret',
    }),
  );
  expect(screen.getByLabelText('Google client secret')).toHaveValue('');
  expect(screen.getByLabelText('Google client ID')).toHaveValue('');
  await waitFor(() =>
    expect(screen.getByRole('button', { name: 'Connect Gmail' })).toBeEnabled(),
  );
  expect(
    screen.getByRole('button', { name: 'Connect Google Calendar' }),
  ).toBeEnabled();
  expect(api.connectMail).not.toHaveBeenCalled();
  expect(screen.queryByText('top-secret')).not.toBeInTheDocument();
});

it('saves Microsoft credentials with tenant and enables Outlook independently', async () => {
  let outlookCalls = 0;
  api.configureOauthApp.mockResolvedValue({
    provider: 'microsoft',
    configured: true,
    source: 'vault',
    clientIdHint: 'mic…-id',
    redirectUris: ['https://daemon.test/microsoft/callback'],
    tenant: 'organizations',
  });
  api.mailStatus.mockImplementation((_id, provider) => {
    if (provider === 'outlook') outlookCalls++;
    return Promise.resolve({
      configured: provider === 'outlook' && outlookCalls > 1,
      connector: null,
    });
  });
  const user = userEvent.setup();
  render(<ConnectorsView agentId="main" telegram={null} />);
  await user.type(
    await screen.findByLabelText('Microsoft client ID'),
    'microsoft-id',
  );
  await user.type(screen.getByLabelText('Microsoft client secret'), 'secret');
  const tenant = screen.getByLabelText('Microsoft tenant');
  await user.clear(tenant);
  await user.type(tenant, 'organizations');
  await user.click(
    screen.getByRole('button', { name: 'Save Microsoft OAuth app' }),
  );

  expect(api.configureOauthApp).toHaveBeenCalledWith('microsoft', {
    clientId: 'microsoft-id',
    clientSecret: 'secret',
    tenant: 'organizations',
  });
  await waitFor(() =>
    expect(
      screen.getByRole('button', { name: 'Connect Outlook' }),
    ).toBeEnabled(),
  );
  expect(screen.getByRole('button', { name: 'Connect Gmail' })).toBeDisabled();
});

it('keeps provider failures independent and validates credentials locally', async () => {
  api.oauthAppStatus.mockImplementation((provider) =>
    provider === 'google'
      ? Promise.reject(new Error('provider secret'))
      : Promise.resolve({
          provider,
          configured: false,
          source: null,
          clientIdHint: null,
          redirectUris: ['https://daemon.test/microsoft/callback'],
          tenant: 'common',
        }),
  );
  const user = userEvent.setup();
  render(
    <ConnectorsView agentId="main" telegram={<div>Telegram management</div>} />,
  );

  expect(
    await screen.findByText('Could not refresh this connection. Try again.'),
  ).toBeVisible();
  expect(screen.getByText('Microsoft OAuth app')).toBeVisible();
  expect(screen.getByText('Telegram management')).toBeVisible();
  await user.type(screen.getByLabelText('Microsoft client ID'), 'id-only');
  await user.click(
    screen.getByRole('button', { name: 'Save Microsoft OAuth app' }),
  );
  expect(
    await screen.findByText('Client ID and client secret are required.'),
  ).toBeVisible();
  expect(api.configureOauthApp).not.toHaveBeenCalled();
  expect(screen.queryByText('provider secret')).not.toBeInTheDocument();
});

it('removes vault credentials and treats environment credentials as managed outside the UI', async () => {
  api.oauthAppStatus.mockImplementation((provider) =>
    Promise.resolve({
      provider,
      configured: true,
      source: provider === 'google' ? 'vault' : 'environment',
      clientIdHint: provider === 'google' ? 'goo…123' : 'mic…456',
      redirectUris: [`https://daemon.test/${provider}/callback`],
      tenant: provider === 'microsoft' ? 'common' : null,
    }),
  );
  api.removeOauthApp.mockRejectedValue(new Error('connector_conflict secret'));
  const user = userEvent.setup();
  render(<ConnectorsView agentId="main" telegram={null} />);

  expect(await screen.findByText('goo…123')).toBeVisible();
  expect(screen.getByText('mic…456')).toBeVisible();
  expect(
    screen.getByText('Managed through the daemon environment.'),
  ).toBeVisible();
  expect(
    screen.queryByRole('button', { name: 'Remove Microsoft OAuth app' }),
  ).not.toBeInTheDocument();
  expect(screen.getByLabelText('Microsoft client ID')).toBeDisabled();
  await user.click(
    screen.getByRole('button', { name: 'Remove Google OAuth app' }),
  );
  expect(api.removeOauthApp).toHaveBeenCalledWith('google');
  expect(
    await screen.findByText(
      'This action could not be completed. Refresh the connection before trying again.',
    ),
  ).toBeVisible();
  expect(
    screen.queryByText(/connector_conflict secret/),
  ).not.toBeInTheDocument();
});
