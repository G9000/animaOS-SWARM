import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { daemon, type DaemonProvider } from '../../lib/daemon-api';
import { CompanionSetup } from './CompanionSetup';

const providers: DaemonProvider[] = [
  {
    id: 'openai',
    label: 'OpenAI',
    configured: true,
    requiresKey: true,
    apiKeyEnvs: ['OPENAI_API_KEY'],
  },
];
afterEach(() => vi.restoreAllMocks());

describe('CompanionSetup', () => {
  it.each([true, false])(
    'never defaults to the mock provider (real provider configured: %s)',
    async (configured) => {
      vi.spyOn(daemon, 'getWorkspace').mockResolvedValue({
        configured: false,
        workspace: null,
        defaultRoot: '/data/workspace',
      });
      const create = vi
        .spyOn(daemon, 'bootstrapWorkspace')
        .mockResolvedValue({ agent: { state: { id: 'main' } } } as never);
      render(
        <CompanionSetup
          providers={[
            {
              id: 'deterministic',
              label: 'Deterministic (mock)',
              configured: true,
              requiresKey: false,
              apiKeyEnvs: [],
            },
            {
              id: 'ollama',
              label: 'Ollama',
              configured: true,
              requiresKey: false,
              apiKeyEnvs: [],
            },
            { ...providers[0], configured },
          ]}
          providersError={null}
          retryProviders={vi.fn()}
          onCreated={vi.fn()}
        />,
      );
      await waitFor(() =>
        expect(
          screen.queryByText('Loading your workspace…'),
        ).not.toBeInTheDocument(),
      );
      const start = screen.getByRole('button', { name: 'Start chatting' });
      if (configured) {
        await userEvent.click(start);
        expect(create.mock.calls[0][0].agent.provider).toBe('openai');
      } else {
        expect(start).toBeDisabled();
        expect(create).not.toHaveBeenCalled();
      }
    },
  );

  it('creates exactly one companion using the daemon workspace directory, with no agency generation', async () => {
    vi.spyOn(daemon, 'getWorkspace').mockResolvedValue({
      configured: false,
      workspace: null,
      defaultRoot: '/data/workspace',
    });
    const create = vi
      .spyOn(daemon, 'bootstrapWorkspace')
      .mockResolvedValue({ agent: { state: { id: 'main' } } } as never);
    const generate = vi.spyOn(daemon, 'generateAgency');
    const created = vi.fn();
    render(
      <CompanionSetup
        providers={providers}
        providersError={null}
        retryProviders={vi.fn()}
        onCreated={created}
      />,
    );
    await waitFor(() =>
      expect(
        screen.getByRole('button', { name: 'Start chatting' }),
      ).toBeEnabled(),
    );
    expect(screen.queryByText('Team')).not.toBeInTheDocument();
    await userEvent.click(
      screen.getByRole('button', { name: 'Start chatting' }),
    );
    expect(create).toHaveBeenCalledOnce();
    expect(create.mock.calls[0][0]).toMatchObject({
      workspace: {
        rootPath: '/data/workspace',
        companyName: 'Personal',
        values: [],
      },
      agent: { name: 'Anima', provider: 'openai' },
    });
    expect(create.mock.calls[0][0]).not.toHaveProperty('workers');
    expect(create.mock.calls[0][0].agent.system).toContain(
      'personal companion',
    );
    expect(generate).not.toHaveBeenCalled();
    expect(created).toHaveBeenCalledOnce();
  });

  it('does not bootstrap over a workspace that failed to load', async () => {
    vi.spyOn(daemon, 'getWorkspace').mockRejectedValue(
      new Error('Storage unavailable'),
    );
    const create = vi.spyOn(daemon, 'bootstrapWorkspace');
    render(
      <CompanionSetup
        providers={providers}
        providersError={null}
        retryProviders={vi.fn()}
        onCreated={vi.fn()}
      />,
    );
    expect(await screen.findByText('Storage unavailable')).toBeVisible();
    expect(
      screen.getByRole('button', { name: 'Start chatting' }),
    ).toBeDisabled();
    expect(create).not.toHaveBeenCalled();
    expect(
      screen.getByRole('button', { name: 'Retry workspace' }),
    ).toBeVisible();
  });

  it('reuses configured storage without overwriting its mission or existing agents', async () => {
    vi.spyOn(daemon, 'getWorkspace').mockResolvedValue({
      configured: true,
      workspace: {
        rootPath: '/saved',
        companyName: 'Saved',
        mission: 'Keep me',
        values: [],
        hasAvatar: false,
      },
      defaultRoot: '/data/workspace',
    });
    const bootstrap = vi.spyOn(daemon, 'bootstrapWorkspace');
    const create = vi
      .spyOn(daemon, 'createAgent')
      .mockResolvedValue({ agent: { state: { id: 'main' } } } as never);
    render(
      <CompanionSetup
        providers={providers}
        providersError={null}
        retryProviders={vi.fn()}
        onCreated={vi.fn()}
      />,
    );
    await waitFor(() =>
      expect(
        screen.getByRole('button', { name: 'Start chatting' }),
      ).toBeEnabled(),
    );
    await userEvent.click(
      screen.getByRole('button', { name: 'Start chatting' }),
    );
    expect(bootstrap).not.toHaveBeenCalled();
    expect(create).toHaveBeenCalledWith(
      expect.objectContaining({
        settings: { additional: { workspaceRole: 'lead' } },
      }),
    );
  });

  it('blocks submission when a selected provider becomes unavailable', async () => {
    vi.spyOn(daemon, 'getWorkspace').mockResolvedValue({
      configured: false,
      workspace: null,
      defaultRoot: '/data/workspace',
    });
    const props = {
      providers,
      providersError: null,
      retryProviders: vi.fn(),
      onCreated: vi.fn(),
    };
    const view = render(<CompanionSetup {...props} />);
    await waitFor(() =>
      expect(
        screen.getByRole('button', { name: 'Start chatting' }),
      ).toBeEnabled(),
    );
    view.rerender(
      <CompanionSetup
        {...props}
        providers={[{ ...providers[0], configured: false }]}
      />,
    );
    expect(
      screen.getByRole('button', { name: 'Start chatting' }),
    ).toBeDisabled();
  });
});
