import { StrictMode } from 'react';
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { toolNamesForProfile } from '../../lib/agent-access';
import { CHECKIN_SENTINEL } from '../../lib/checkins';
import {
  daemon,
  type DaemonProvider,
  type DaemonSnapshot,
} from '../../lib/daemon-api';
import { ViewHarness } from '../../ViewHarness';
import { OnboardingFlow } from './OnboardingFlow';

const nativeSetTimeout = window.setTimeout.bind(window);

const configuredProviders: DaemonProvider[] = [
  {
    id: 'openai',
    label: 'OpenAI',
    requiresKey: true,
    configured: true,
    apiKeyEnvs: ['OPENAI_API_KEY'],
  },
  {
    id: 'anthropic',
    label: 'Anthropic',
    requiresKey: true,
    configured: false,
    apiKeyEnvs: ['ANTHROPIC_API_KEY'],
  },
  {
    id: 'ollama',
    label: 'Ollama',
    requiresKey: false,
    configured: true,
    apiKeyEnvs: [],
  },
];

function snapshot(): DaemonSnapshot {
  return {
    state: {
      id: 'agent-1',
      name: 'Nova',
      status: 'idle',
      config: {
        name: 'Nova',
        provider: 'openai',
        model: 'gpt-4.1',
        system: 'Be precise',
        tools: [],
      },
      createdAtMs: 1,
      tokenUsage: {
        promptTokens: 0,
        completionTokens: 0,
        totalTokens: 0,
      },
    },
    messageCount: 0,
    messages: [],
    eventCount: 0,
  };
}

function namedSnapshot(name: string, id: string): DaemonSnapshot {
  const base = snapshot();
  return {
    ...base,
    state: {
      ...base.state,
      id,
      name,
      config: { ...base.state.config, name },
    },
  };
}

function snapshotWithReply(text: string): DaemonSnapshot {
  const base = snapshot();
  return {
    ...base,
    messageCount: 2,
    messages: [
      {
        id: 'message-user',
        agentId: base.state.id,
        roomId: 'room-1',
        content: { text: 'Hello' },
        role: 'user',
        createdAtMs: 2,
      },
      {
        id: 'message-assistant',
        agentId: base.state.id,
        roomId: 'room-1',
        content: { text },
        role: 'assistant',
        createdAtMs: 3,
      },
    ],
  };
}

function namedSnapshotWithReply(
  name: string,
  id: string,
  text: string,
): DaemonSnapshot {
  const base = namedSnapshot(name, id);
  return {
    ...base,
    messageCount: 2,
    messages: [
      {
        id: 'message-user',
        agentId: id,
        roomId: 'room-1',
        content: { text: 'New work' },
        role: 'user',
        createdAtMs: 2,
      },
      {
        id: 'message-assistant',
        agentId: id,
        roomId: 'room-1',
        content: { text },
        role: 'assistant',
        createdAtMs: 3,
      },
    ],
  };
}

function deferred<Value>() {
  let resolve!: (value: Value) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<Value>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, reject, resolve };
}

function renderFlow(
  props: Partial<React.ComponentProps<typeof OnboardingFlow>> = {},
) {
  const onCreated = vi.fn();
  const retryProviders = vi.fn();
  const result = render(
    <OnboardingFlow
      providers={configuredProviders}
      providersError={null}
      retryProviders={retryProviders}
      onCreated={onCreated}
      {...props}
    />,
  );

  return { ...result, onCreated, retryProviders };
}

async function goToIntelligence(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole('button', { name: 'Next' }));
  expect(screen.getByRole('heading', { name: 'Intelligence' })).toBeVisible();
}

async function goToAccess(user: ReturnType<typeof userEvent.setup>) {
  await goToIntelligence(user);
  await user.click(screen.getByRole('button', { name: 'Next' }));
  expect(screen.getByRole('heading', { name: 'Access' })).toBeVisible();
}

async function goToReview(user: ReturnType<typeof userEvent.setup>) {
  await goToAccess(user);
  await user.click(screen.getByRole('button', { name: 'Next' }));
  expect(screen.getByRole('heading', { name: 'Review' })).toBeVisible();
}

beforeEach(() => {
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
});

afterEach(() => {
  localStorage.clear();
  vi.restoreAllMocks();
});

describe('OnboardingFlow', () => {
  it('defaults the identity, persists optional instructions, and focuses a blank required name', async () => {
    const user = userEvent.setup();
    renderFlow();

    const name = screen.getByRole('textbox', { name: 'Agent name' });
    const instructions = screen.getByRole('textbox', {
      name: 'Instructions (optional)',
    });
    expect(name).toHaveValue('Anima');
    expect(name).toBeRequired();
    expect(name).toHaveAttribute('aria-invalid', 'false');
    expect(name).not.toHaveAttribute('aria-describedby');

    await user.clear(name);
    await user.type(name, '   ');
    await user.type(instructions, 'Keep answers short.');
    await user.click(screen.getByRole('button', { name: 'Next' }));

    expect(screen.getByRole('heading', { name: 'Identity' })).toBeVisible();
    const nameAlert = screen.getByRole('alert');
    expect(nameAlert).toHaveTextContent('Enter an agent name.');
    expect(nameAlert).toHaveAttribute('id', 'onboarding-agent-name-error');
    expect(name).toHaveAttribute('aria-invalid', 'true');
    expect(name).toHaveAttribute(
      'aria-describedby',
      'onboarding-agent-name-error',
    );
    expect(name).toHaveFocus();
    expect(screen.getAllByRole('listitem')[0]).toHaveAttribute(
      'aria-current',
      'step',
    );

    await user.clear(name);
    await user.type(name, 'Anima Prime');
    expect(name).toHaveAttribute('aria-invalid', 'false');
    expect(name).not.toHaveAttribute('aria-describedby');
    await user.click(screen.getByRole('button', { name: 'Next' }));
    await user.click(screen.getByRole('button', { name: 'Back' }));

    expect(screen.getByRole('textbox', { name: 'Agent name' })).toHaveValue(
      'Anima Prime',
    );
    expect(
      screen.getByRole('textbox', { name: 'Instructions (optional)' }),
    ).toHaveValue('Keep answers short.');
  });

  it('updates semantic progress and the polite announcement on every step change', async () => {
    const user = userEvent.setup();
    renderFlow();

    const expectedSteps = ['Identity', 'Intelligence', 'Access', 'Review'];
    for (let index = 0; index < expectedSteps.length; index += 1) {
      const progressItems = screen.getAllByRole('listitem');
      expect(progressItems[index]).toHaveAttribute('aria-current', 'step');
      expect(
        progressItems.filter((item) => item.hasAttribute('aria-current')),
      ).toEqual([progressItems[index]]);
      expect(screen.getByRole('status')).toHaveTextContent(
        `Step ${index + 1} of 4: ${expectedSteps[index]}`,
      );

      if (index < expectedSteps.length - 1) {
        await user.click(screen.getByRole('button', { name: 'Next' }));
      }
    }

    for (let index = expectedSteps.length - 2; index >= 0; index -= 1) {
      await user.click(screen.getByRole('button', { name: 'Back' }));
      expect(screen.getAllByRole('listitem')[index]).toHaveAttribute(
        'aria-current',
        'step',
      );
      expect(screen.getByRole('status')).toHaveTextContent(
        `Step ${index + 1} of 4: ${expectedSteps[index]}`,
      );
    }
  });

  it('shows every provider, disables unavailable providers with env guidance, and validates custom models', async () => {
    const user = userEvent.setup();
    renderFlow();
    await goToIntelligence(user);

    expect(
      screen.getByRole('group', { name: 'Provider catalog' }),
    ).toBeVisible();
    const openai = screen.getByRole('button', {
      name: /OpenAI.*configured/i,
    });
    expect(openai).toBeEnabled();
    expect(openai).toHaveFocus();
    expect(openai).toHaveAttribute('aria-pressed', 'true');
    expect(
      screen.getByRole('button', { name: /Anthropic.*unavailable/i }),
    ).toBeDisabled();
    expect(
      screen.getByRole('button', { name: /Ollama.*configured/i }),
    ).toBeEnabled();
    expect(
      screen.getByText(/Set ANTHROPIC_API_KEY in the daemon environment/i),
    ).toBeVisible();

    const ollama = screen.getByRole('button', {
      name: /Ollama.*configured/i,
    });
    await user.click(ollama);
    expect(ollama).toHaveAttribute('aria-pressed', 'true');
    expect(openai).toHaveAttribute('aria-pressed', 'false');
    expect(screen.getByRole('combobox', { name: 'Model' })).toHaveValue(
      'llama3.1',
    );
    expect(screen.getByRole('option', { name: 'qwen2.5' })).toBeVisible();

    await user.selectOptions(
      screen.getByRole('combobox', { name: 'Model' }),
      '__custom__',
    );
    const customModel = screen.getByRole('textbox', { name: 'Custom model' });
    expect(customModel).toBeRequired();
    expect(customModel).toHaveAttribute('aria-invalid', 'false');
    expect(customModel).not.toHaveAttribute('aria-describedby');
    await user.type(customModel, '   ');
    await user.click(screen.getByRole('button', { name: 'Next' }));

    expect(screen.getByRole('heading', { name: 'Intelligence' })).toBeVisible();
    const modelAlert = screen.getByRole('alert');
    expect(modelAlert).toHaveTextContent('Enter a model.');
    expect(modelAlert).toHaveAttribute('id', 'onboarding-custom-model-error');
    expect(customModel).toHaveAttribute('aria-invalid', 'true');
    expect(customModel).toHaveAttribute(
      'aria-describedby',
      'onboarding-custom-model-error',
    );
    expect(customModel).toHaveFocus();

    await user.type(customModel, 'llama3.2');
    expect(customModel).toHaveAttribute('aria-invalid', 'false');
    expect(customModel).not.toHaveAttribute('aria-describedby');
    await user.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByRole('heading', { name: 'Access' })).toBeVisible();
  });

  it('makes provider loading and retry explicit without discarding the identity draft', async () => {
    const user = userEvent.setup();
    const retry = deferred<void>();
    const retryProviders = vi.fn(() => retry.promise);
    const view = renderFlow({
      providers: null,
      providersError: null,
      retryProviders,
    });

    const name = screen.getByRole('textbox', { name: 'Agent name' });
    await user.clear(name);
    await user.type(name, 'Persistent Anima');
    await goToIntelligence(user);
    expect(screen.getByText('Loading provider catalog…')).toHaveAttribute(
      'role',
      'status',
    );
    expect(screen.getByLabelText('Provider catalog')).toHaveAttribute(
      'aria-busy',
      'true',
    );
    expect(screen.getByRole('button', { name: 'Next' })).toBeDisabled();

    view.rerender(
      <OnboardingFlow
        providers={null}
        providersError="provider catalog failed"
        retryProviders={retryProviders}
        onCreated={vi.fn()}
      />,
    );
    expect(screen.getByRole('alert')).toHaveTextContent(
      'provider catalog failed',
    );
    const retryButton = screen.getByRole('button', {
      name: 'Retry providers',
    });
    expect(retryButton).toHaveFocus();
    expect(screen.getByRole('button', { name: 'Next' })).toBeDisabled();
    expect(
      screen.queryByText('Choose a configured provider.'),
    ).not.toBeInTheDocument();
    act(() => {
      retryButton.click();
      retryButton.click();
    });
    expect(retryProviders).toHaveBeenCalledTimes(1);
    expect(screen.getByText('Retrying provider catalog…')).toHaveAttribute(
      'role',
      'status',
    );
    expect(screen.getByRole('button', { name: 'Next' })).toBeDisabled();

    await act(async () => {
      view.rerender(
        <OnboardingFlow
          providers={configuredProviders}
          providersError={null}
          retryProviders={retryProviders}
          onCreated={vi.fn()}
        />,
      );
      retry.resolve();
      await retry.promise;
    });
    expect(screen.getByRole('button', { name: 'Next' })).toBeEnabled();
    await user.click(screen.getByRole('button', { name: 'Back' }));
    expect(screen.getByRole('textbox', { name: 'Agent name' })).toHaveValue(
      'Persistent Anima',
    );
  });

  it('explains when no providers are configured and routes focus to retry', async () => {
    const user = userEvent.setup();
    const unavailableProviders: DaemonProvider[] = [
      {
        id: 'anthropic',
        label: 'Anthropic',
        requiresKey: true,
        configured: false,
        apiKeyEnvs: ['ANTHROPIC_API_KEY'],
      },
    ];
    renderFlow({ providers: unavailableProviders });

    await goToIntelligence(user);

    expect(
      screen.getByText(
        'No providers are configured. Add a provider credential to the daemon environment, then retry.',
      ),
    ).toBeVisible();
    expect(
      screen.getByRole('button', { name: /Anthropic.*unavailable/i }),
    ).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Next' })).toBeDisabled();
    expect(
      screen.getByRole('button', { name: 'Retry providers' }),
    ).toHaveFocus();
  });

  it('catches a provider retry rejection, announces it, and preserves Identity', async () => {
    const user = userEvent.setup();
    const retryProviders = vi
      .fn()
      .mockRejectedValue(new Error('retry transport failed'));
    renderFlow({
      providers: null,
      providersError: 'provider catalog failed',
      retryProviders,
    });

    const name = screen.getByRole('textbox', { name: 'Agent name' });
    await user.clear(name);
    await user.type(name, 'Still Anima');
    await goToIntelligence(user);
    await user.click(screen.getByRole('button', { name: 'Retry providers' }));

    await screen.findByText('retry transport failed');
    const retryAlert = screen.getByRole('alert');
    expect(retryAlert).toHaveTextContent('retry transport failed');
    expect(
      screen.getByRole('button', { name: 'Retry providers' }),
    ).toHaveFocus();
    expect(screen.getByRole('button', { name: 'Next' })).toBeDisabled();

    await user.click(screen.getByRole('button', { name: 'Back' }));
    expect(screen.getByRole('textbox', { name: 'Agent name' })).toHaveValue(
      'Still Anima',
    );
  });

  it('installs a deterministic configured fallback and clears stale model validation', async () => {
    const user = userEvent.setup();
    const customProvider: DaemonProvider = {
      id: 'custom-provider',
      label: 'Custom provider',
      requiresKey: false,
      configured: true,
      apiKeyEnvs: [],
    };
    const view = renderFlow({ providers: [customProvider] });
    await goToIntelligence(user);

    const customModel = screen.getByRole('textbox', { name: 'Custom model' });
    await user.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByRole('alert')).toHaveTextContent('Enter a model.');
    expect(customModel).toHaveFocus();

    view.rerender(
      <OnboardingFlow
        providers={[
          { ...customProvider, configured: false },
          configuredProviders[2],
        ]}
        providersError={null}
        retryProviders={vi.fn()}
        onCreated={vi.fn()}
      />,
    );

    const ollama = await screen.findByRole('button', {
      name: /Ollama.*configured/i,
    });
    await waitFor(() => expect(ollama).toHaveAttribute('aria-pressed', 'true'));
    expect(ollama).toHaveFocus();
    expect(screen.getByRole('combobox', { name: 'Model' })).toHaveValue(
      'llama3.1',
    );
    expect(screen.queryByText('Enter a model.')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Next' })).toBeEnabled();
  });

  it('defaults to Collaborate and explains every access profile in text', async () => {
    const user = userEvent.setup();
    renderFlow();
    await goToAccess(user);

    expect(screen.getByRole('radio', { name: /Collaborate/ })).toBeChecked();
    expect(
      screen.getByText('Inspect workspace files and todos.'),
    ).toBeVisible();
    expect(
      screen.getByText(
        'Read-only workspace access; cannot modify files or execute processes.',
      ),
    ).toBeVisible();
    expect(
      screen.getByText('Inspect and update workspace files and todos.'),
    ).toBeVisible();
    expect(
      screen.getByText('Can modify workspace files; cannot execute processes.'),
    ).toBeVisible();
    expect(
      screen.getByText('Inspect, update, and run work in the workspace.'),
    ).toBeVisible();
    expect(
      screen.getByText(
        'Can execute shell commands and manage background processes.',
      ),
    ).toBeVisible();

    await user.click(screen.getByRole('radio', { name: /Observe/ }));
    expect(screen.getByRole('radio', { name: /Observe/ })).toBeChecked();
    await user.click(screen.getByRole('radio', { name: /Operate/ }));
    expect(screen.getByRole('radio', { name: /Operate/ })).toBeChecked();
  });

  it('summarizes the complete draft and preserves every value when moving back', async () => {
    const user = userEvent.setup();
    renderFlow();

    const name = screen.getByRole('textbox', { name: 'Agent name' });
    await user.clear(name);
    await user.type(name, 'Nova');
    await user.type(
      screen.getByRole('textbox', { name: 'Instructions (optional)' }),
      'Be precise',
    );
    await goToIntelligence(user);
    await user.selectOptions(
      screen.getByRole('combobox', { name: 'Model' }),
      '__custom__',
    );
    await user.type(
      screen.getByRole('textbox', { name: 'Custom model' }),
      'custom/great-model',
    );
    await user.click(screen.getByRole('button', { name: 'Next' }));
    await user.click(screen.getByRole('radio', { name: /Operate/ }));
    await user.click(screen.getByRole('button', { name: 'Next' }));

    expect(screen.getByText('Nova')).toBeVisible();
    expect(screen.getByText('OpenAI / custom/great-model')).toBeVisible();
    expect(screen.getByText('Operate')).toBeVisible();
    expect(
      screen.getByText(
        'Can execute shell commands and manage background processes.',
      ),
    ).toBeVisible();

    await user.click(screen.getByRole('button', { name: 'Back' }));
    expect(screen.getByRole('radio', { name: /Operate/ })).toBeChecked();
    await user.click(screen.getByRole('button', { name: 'Back' }));
    expect(
      screen.getByRole('button', { name: /OpenAI.*configured/i }),
    ).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByRole('combobox', { name: 'Model' })).toHaveValue(
      '__custom__',
    );
    expect(screen.getByRole('textbox', { name: 'Custom model' })).toHaveValue(
      'custom/great-model',
    );
    await user.click(screen.getByRole('button', { name: 'Back' }));
    expect(screen.getByRole('textbox', { name: 'Agent name' })).toHaveValue(
      'Nova',
    );
    expect(
      screen.getByRole('textbox', { name: 'Instructions (optional)' }),
    ).toHaveValue('Be precise');
  });

  it('submits the exact resolved draft once and hands off the POST snapshot without listing agents', async () => {
    const user = userEvent.setup();
    const created = snapshot();
    const createAgent = vi
      .spyOn(daemon, 'createAgent')
      .mockResolvedValue({ agent: created });
    const listAgents = vi.spyOn(daemon, 'listAgents');
    const onCreated = vi.fn();
    renderFlow({ onCreated });

    const name = screen.getByRole('textbox', { name: 'Agent name' });
    await user.clear(name);
    await user.type(name, '  Nova  ');
    await user.type(
      screen.getByRole('textbox', { name: 'Instructions (optional)' }),
      '  Be precise  ',
    );
    await goToIntelligence(user);
    await user.selectOptions(
      screen.getByRole('combobox', { name: 'Model' }),
      'gpt-4.1',
    );
    await user.click(screen.getByRole('button', { name: 'Next' }));
    await user.click(screen.getByRole('radio', { name: /Operate/ }));
    await user.click(screen.getByRole('button', { name: 'Next' }));
    await user.click(screen.getByRole('button', { name: 'Create agent' }));

    await waitFor(() => expect(onCreated).toHaveBeenCalledWith(created));
    expect(createAgent).toHaveBeenCalledTimes(1);
    expect(createAgent).toHaveBeenCalledWith({
      name: 'Nova',
      provider: 'openai',
      model: 'gpt-4.1',
      system: 'Be precise',
      tools: toolNamesForProfile('operate'),
    });
    expect(listAgents).not.toHaveBeenCalled();
  });

  it('does not publish a deferred create after onboarding unmounts', async () => {
    const user = userEvent.setup();
    const create = deferred<{ agent: DaemonSnapshot }>();
    vi.spyOn(daemon, 'createAgent').mockReturnValue(create.promise);
    const onCreated = vi.fn();
    const view = renderFlow({ onCreated });

    await goToReview(user);
    await user.click(screen.getByRole('button', { name: 'Create agent' }));
    await waitFor(() => expect(daemon.createAgent).toHaveBeenCalledTimes(1));
    view.unmount();

    await act(async () => {
      create.resolve({ agent: snapshot() });
      await create.promise;
    });

    expect(onCreated).not.toHaveBeenCalled();
  });

  it('returns Review to Intelligence when the reviewed provider is invalidated', async () => {
    const user = userEvent.setup();
    const createAgent = vi.spyOn(daemon, 'createAgent');
    const view = renderFlow();
    await goToReview(user);
    expect(screen.getByText('OpenAI / gpt-4o')).toBeVisible();

    view.rerender(
      <OnboardingFlow
        providers={[
          { ...configuredProviders[0], configured: false },
          configuredProviders[2],
        ]}
        providersError={null}
        retryProviders={vi.fn()}
        onCreated={view.onCreated}
      />,
    );

    expect(
      await screen.findByRole('heading', { name: 'Intelligence' }),
    ).toBeVisible();
    expect(screen.getByRole('alert')).toHaveTextContent(
      'Provider catalog changed. Review your provider and model before creating the agent.',
    );
    const ollama = screen.getByRole('button', {
      name: /Ollama.*configured/i,
    });
    await waitFor(() => expect(ollama).toHaveAttribute('aria-pressed', 'true'));
    expect(ollama).toHaveFocus();
    expect(
      screen.queryByRole('button', { name: 'Create agent' }),
    ).not.toBeInTheDocument();
    expect(createAgent).not.toHaveBeenCalled();
  });

  it('keeps a rejected non-default draft intact and prevents double submit on retry', async () => {
    const user = userEvent.setup();
    const firstCreate = deferred<{ agent: DaemonSnapshot }>();
    const secondCreate = deferred<{ agent: DaemonSnapshot }>();
    const createAgent = vi
      .spyOn(daemon, 'createAgent')
      .mockReturnValueOnce(firstCreate.promise)
      .mockReturnValueOnce(secondCreate.promise);
    const onCreated = vi.fn();
    renderFlow({ onCreated });

    const name = screen.getByRole('textbox', { name: 'Agent name' });
    await user.clear(name);
    await user.type(name, 'Nova');
    await user.type(
      screen.getByRole('textbox', { name: 'Instructions (optional)' }),
      'Be precise',
    );
    await goToIntelligence(user);
    await user.selectOptions(
      screen.getByRole('combobox', { name: 'Model' }),
      '__custom__',
    );
    await user.type(
      screen.getByRole('textbox', { name: 'Custom model' }),
      'custom/great-model',
    );
    await user.click(screen.getByRole('button', { name: 'Next' }));
    await user.click(screen.getByRole('radio', { name: /Operate/ }));
    await user.click(screen.getByRole('button', { name: 'Next' }));
    await user.click(screen.getByRole('button', { name: 'Create agent' }));
    const creatingButton = screen.getByRole('button', {
      name: 'Creating agent…',
    });
    expect(creatingButton).toBeDisabled();
    await user.click(creatingButton);
    expect(createAgent).toHaveBeenCalledTimes(1);

    await act(async () => {
      firstCreate.reject(new Error('daemon refused creation'));
    });

    const createAlert = await screen.findByRole('alert');
    expect(createAlert).toHaveTextContent('daemon refused creation');
    expect(createAlert).toHaveFocus();
    expect(screen.getByRole('heading', { name: 'Review' })).toBeVisible();
    expect(screen.getByText('Nova')).toBeVisible();
    expect(screen.getByText('OpenAI / custom/great-model')).toBeVisible();
    expect(screen.getByText('Operate')).toBeVisible();
    expect(screen.getByText('Be precise')).toBeVisible();

    await user.click(screen.getByRole('button', { name: 'Create agent' }));
    expect(createAgent).toHaveBeenCalledTimes(2);
    await user.click(screen.getByRole('button', { name: 'Creating agent…' }));
    expect(createAgent).toHaveBeenCalledTimes(2);
    await act(async () => {
      secondCreate.resolve({ agent: snapshot() });
    });
    await waitFor(() => expect(onCreated).toHaveBeenCalledWith(snapshot()));
    expect(createAgent).toHaveBeenCalledTimes(2);
  });

  it('settles a settings save from its snapshot while a slow poll continues', async () => {
    const user = userEvent.setup();
    const slowPoll = deferred<{ agents: DaemonSnapshot[] }>();
    const listAgents = vi
      .spyOn(daemon, 'listAgents')
      .mockResolvedValueOnce({ agents: [snapshot()] })
      .mockReturnValue(slowPoll.promise);
    vi.spyOn(daemon, 'listProviders').mockResolvedValue({
      providers: configuredProviders,
    });
    const updated = namedSnapshot('Nova Saved', 'agent-1');
    const updateAgent = vi
      .spyOn(daemon, 'updateAgent')
      .mockResolvedValue({ agent: updated });
    let runPoll: (() => void) | undefined;
    vi.spyOn(window, 'setTimeout').mockImplementation(((
      handler: TimerHandler,
      timeout?: number,
    ) => {
      if (typeof handler === 'function' && timeout === 5_000) {
        runPoll = handler;
        return 1;
      }
      return nativeSetTimeout(handler, timeout);
    }) as typeof window.setTimeout);

    render(<ViewHarness />);
    expect(
      await screen.findByRole('heading', { name: 'Say something to Nova' }),
    ).toBeVisible();

    act(() => {
      runPoll?.();
    });
    expect(listAgents).toHaveBeenCalledTimes(2);

    await user.click(screen.getAllByRole('button', { name: 'Settings' })[0]);
    const name = screen.getByDisplayValue('Nova');
    await user.clear(name);
    await user.type(name, 'Nova Saved');
    await user.click(screen.getByRole('button', { name: 'Save changes' }));

    expect(updateAgent).toHaveBeenCalledWith('agent-1', {
      name: 'Nova Saved',
    });
    expect(
      await screen.findByRole('button', { name: 'Saved ✓' }),
    ).toBeVisible();
    expect(screen.getByDisplayValue('Nova Saved')).toBeVisible();
    expect(listAgents).toHaveBeenCalledTimes(2);
  });

  it('settles a chat run from its snapshot while a slow poll continues', async () => {
    const user = userEvent.setup();
    const slowPoll = deferred<{ agents: DaemonSnapshot[] }>();
    const listAgents = vi
      .spyOn(daemon, 'listAgents')
      .mockResolvedValueOnce({ agents: [snapshot()] })
      .mockReturnValue(slowPoll.promise);
    vi.spyOn(daemon, 'listProviders').mockResolvedValue({
      providers: configuredProviders,
    });
    vi.spyOn(daemon, 'runAgent').mockResolvedValue({
      agent: snapshotWithReply('Hello back'),
      result: {
        status: 'success',
        durationMs: 1,
        data: { text: 'Hello back' },
      },
    });
    let runPoll: (() => void) | undefined;
    vi.spyOn(window, 'setTimeout').mockImplementation(((
      handler: TimerHandler,
      timeout?: number,
    ) => {
      if (typeof handler === 'function' && timeout === 5_000) {
        runPoll = handler;
        return 1;
      }
      return nativeSetTimeout(handler, timeout);
    }) as typeof window.setTimeout);

    render(<ViewHarness />);
    expect(
      await screen.findByRole('heading', { name: 'Say something to Nova' }),
    ).toBeVisible();

    act(() => {
      runPoll?.();
    });
    expect(listAgents).toHaveBeenCalledTimes(2);

    await user.type(screen.getByPlaceholderText('Message Nova…'), 'Hello');
    await user.click(screen.getByRole('button', { name: 'Send' }));

    expect(await screen.findByText('Hello back')).toBeVisible();
    await user.type(screen.getByPlaceholderText('Message Nova…'), 'Again');
    expect(screen.getByRole('button', { name: 'Send' })).toBeEnabled();
    expect(listAgents).toHaveBeenCalledTimes(2);
  });

  it('does not resurrect a reset agent when an older chat run resolves', async () => {
    const user = userEvent.setup();
    const pendingRun = deferred<Awaited<ReturnType<typeof daemon.runAgent>>>();
    vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [snapshot()] });
    vi.spyOn(daemon, 'listProviders').mockResolvedValue({
      providers: configuredProviders,
    });
    vi.spyOn(daemon, 'runAgent').mockReturnValue(pendingRun.promise);
    vi.spyOn(daemon, 'deleteAgent').mockResolvedValue({ deleted: true });

    render(<ViewHarness />);
    expect(
      await screen.findByRole('heading', { name: 'Say something to Nova' }),
    ).toBeVisible();

    await user.type(screen.getByPlaceholderText('Message Nova…'), 'Hello');
    await user.click(screen.getByRole('button', { name: 'Send' }));
    await user.click(screen.getAllByRole('button', { name: 'Settings' })[0]);
    await user.click(screen.getByRole('button', { name: 'Reset' }));

    expect(
      await screen.findByRole('heading', { name: 'Create your main agent' }),
    ).toBeVisible();

    await act(async () => {
      pendingRun.resolve({
        agent: namedSnapshot('Resurrected', 'agent-1'),
        result: {
          status: 'success',
          durationMs: 1,
          data: { text: 'Too late' },
        },
      });
      await pendingRun.promise;
    });

    expect(
      screen.getByRole('heading', { name: 'Create your main agent' }),
    ).toBeVisible();
    expect(
      screen.queryByRole('heading', { name: 'Say something to Resurrected' }),
    ).not.toBeInTheDocument();
  });

  it('allows reset only after the pending settings update is adopted', async () => {
    const user = userEvent.setup();
    const pendingUpdate =
      deferred<Awaited<ReturnType<typeof daemon.updateAgent>>>();
    vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [snapshot()] });
    vi.spyOn(daemon, 'listProviders').mockResolvedValue({
      providers: configuredProviders,
    });
    vi.spyOn(daemon, 'updateAgent').mockReturnValue(pendingUpdate.promise);
    vi.spyOn(daemon, 'deleteAgent').mockResolvedValue({ deleted: true });

    render(<ViewHarness />);
    expect(
      await screen.findByRole('heading', { name: 'Say something to Nova' }),
    ).toBeVisible();

    await user.click(screen.getAllByRole('button', { name: 'Settings' })[0]);
    const name = screen.getByDisplayValue('Nova');
    await user.clear(name);
    await user.type(name, 'Stale Saved');
    await user.click(screen.getByRole('button', { name: 'Save changes' }));
    expect(screen.getByRole('button', { name: 'Reset' })).toBeDisabled();
    expect(daemon.deleteAgent).not.toHaveBeenCalled();

    await act(async () => {
      pendingUpdate.resolve({
        agent: namedSnapshot('Stale Saved', 'agent-1'),
      });
      await pendingUpdate.promise;
    });

    expect(await screen.findByDisplayValue('Stale Saved')).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Reset' }));
    expect(
      await screen.findByRole('heading', { name: 'Create your main agent' }),
    ).toBeVisible();
    expect(screen.queryByDisplayValue('Stale Saved')).not.toBeInTheDocument();
  });

  it('keeps a newer chat run busy when the reset agent run resolves', async () => {
    const user = userEvent.setup();
    const olderRun = deferred<Awaited<ReturnType<typeof daemon.runAgent>>>();
    const newerRun = deferred<Awaited<ReturnType<typeof daemon.runAgent>>>();
    vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [snapshot()] });
    vi.spyOn(daemon, 'listProviders').mockResolvedValue({
      providers: configuredProviders,
    });
    const runAgent = vi
      .spyOn(daemon, 'runAgent')
      .mockReturnValueOnce(olderRun.promise)
      .mockReturnValueOnce(newerRun.promise);
    vi.spyOn(daemon, 'deleteAgent').mockResolvedValue({ deleted: true });
    vi.spyOn(daemon, 'createAgent').mockResolvedValue({
      agent: namedSnapshot('Second', 'agent-2'),
    });

    render(<ViewHarness />);
    expect(
      await screen.findByRole('heading', { name: 'Say something to Nova' }),
    ).toBeVisible();

    await user.type(screen.getByPlaceholderText('Message Nova…'), 'Older run');
    await user.click(screen.getByRole('button', { name: 'Send' }));
    await user.click(screen.getAllByRole('button', { name: 'Settings' })[0]);
    await user.click(screen.getByRole('button', { name: 'Reset' }));
    expect(
      await screen.findByRole('heading', { name: 'Create your main agent' }),
    ).toBeVisible();

    await goToReview(user);
    await user.click(screen.getByRole('button', { name: 'Create agent' }));
    await waitFor(() => expect(daemon.createAgent).toHaveBeenCalledTimes(1));
    expect(
      await screen.findByRole('heading', { name: 'Say something to Second' }),
    ).toBeVisible();

    await user.type(
      screen.getByPlaceholderText('Message Second…'),
      'Newer run',
    );
    expect(screen.getByRole('button', { name: 'Send' })).toBeEnabled();
    await user.click(screen.getByRole('button', { name: 'Send' }));
    await waitFor(() => expect(runAgent).toHaveBeenCalledTimes(2));
    expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled();

    await act(async () => {
      olderRun.resolve({
        agent: namedSnapshot('Nova', 'agent-1'),
        result: {
          status: 'success',
          durationMs: 1,
          data: { text: 'Too late' },
        },
      });
      await olderRun.promise;
    });

    expect(screen.getByPlaceholderText('Message Second…')).toBeVisible();
    expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled();

    await act(async () => {
      newerRun.resolve({
        agent: namedSnapshot('Second', 'agent-2'),
        result: {
          status: 'success',
          durationMs: 1,
          data: { text: 'Current reply' },
        },
      });
      await newerRun.promise;
    });
    expect(
      await screen.findByRole('heading', { name: 'Say something to Second' }),
    ).toBeVisible();
  });

  it('keeps a newer agent settings update busy after the prior update settles', async () => {
    const user = userEvent.setup();
    const olderUpdate =
      deferred<Awaited<ReturnType<typeof daemon.updateAgent>>>();
    const newerUpdate =
      deferred<Awaited<ReturnType<typeof daemon.updateAgent>>>();
    vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [snapshot()] });
    vi.spyOn(daemon, 'listProviders').mockResolvedValue({
      providers: configuredProviders,
    });
    const updateAgent = vi
      .spyOn(daemon, 'updateAgent')
      .mockReturnValueOnce(olderUpdate.promise)
      .mockReturnValueOnce(newerUpdate.promise);
    vi.spyOn(daemon, 'deleteAgent').mockResolvedValue({ deleted: true });
    vi.spyOn(daemon, 'createAgent').mockResolvedValue({
      agent: namedSnapshot('Second', 'agent-2'),
    });

    render(<ViewHarness />);
    expect(
      await screen.findByRole('heading', { name: 'Say something to Nova' }),
    ).toBeVisible();

    await user.click(screen.getAllByRole('button', { name: 'Settings' })[0]);
    const olderName = screen.getByDisplayValue('Nova');
    await user.clear(olderName);
    await user.type(olderName, 'Older saved');
    await user.click(screen.getByRole('button', { name: 'Save changes' }));
    expect(screen.getByRole('button', { name: 'Reset' })).toBeDisabled();
    await act(async () => {
      olderUpdate.resolve({
        agent: namedSnapshot('Older saved', 'agent-1'),
      });
      await olderUpdate.promise;
    });
    expect(await screen.findByDisplayValue('Older saved')).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Reset' }));
    expect(
      await screen.findByRole('heading', { name: 'Create your main agent' }),
    ).toBeVisible();

    await goToReview(user);
    await user.click(screen.getByRole('button', { name: 'Create agent' }));
    expect(
      await screen.findByRole('heading', { name: 'Say something to Second' }),
    ).toBeVisible();
    await user.click(screen.getAllByRole('button', { name: 'Settings' })[0]);
    const newerName = screen.getByDisplayValue('Second');
    await user.clear(newerName);
    await user.type(newerName, 'Second saved');
    expect(screen.getByRole('button', { name: 'Save changes' })).toBeEnabled();
    await user.click(screen.getByRole('button', { name: 'Save changes' }));
    await waitFor(() => expect(updateAgent).toHaveBeenCalledTimes(2));
    expect(screen.getByRole('button', { name: 'Saving…' })).toBeDisabled();

    expect(screen.getByDisplayValue('Second saved')).toBeVisible();
    expect(screen.getByRole('button', { name: 'Saving…' })).toBeDisabled();

    await act(async () => {
      newerUpdate.resolve({
        agent: namedSnapshot('Second saved', 'agent-2'),
      });
      await newerUpdate.promise;
    });
    expect(
      await screen.findByRole('button', { name: 'Saved ✓' }),
    ).toBeVisible();
  });

  it('reports a reset error only in settings after the prior update settles', async () => {
    const user = userEvent.setup();
    const pendingUpdate =
      deferred<Awaited<ReturnType<typeof daemon.updateAgent>>>();
    vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [snapshot()] });
    vi.spyOn(daemon, 'listProviders').mockResolvedValue({
      providers: configuredProviders,
    });
    vi.spyOn(daemon, 'updateAgent').mockReturnValue(pendingUpdate.promise);
    vi.spyOn(daemon, 'deleteAgent').mockRejectedValue(
      new Error('reset failed'),
    );

    render(<ViewHarness />);
    expect(
      await screen.findByRole('heading', { name: 'Say something to Nova' }),
    ).toBeVisible();

    await user.click(screen.getAllByRole('button', { name: 'Settings' })[0]);
    const name = screen.getByDisplayValue('Nova');
    await user.clear(name);
    await user.type(name, 'Stale Saved');
    await user.click(screen.getByRole('button', { name: 'Save changes' }));

    await act(async () => {
      pendingUpdate.resolve({
        agent: namedSnapshot('Stale Saved', 'agent-1'),
      });
      await pendingUpdate.promise;
    });

    expect(await screen.findByDisplayValue('Stale Saved')).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Reset' }));
    expect(await screen.findByText('reset failed')).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Close settings' }));
    expect(
      screen.getByRole('heading', { name: 'Say something to Stale Saved' }),
    ).toBeVisible();
    expect(screen.queryByText('reset failed')).not.toBeInTheDocument();
  });

  it('completes deferred reset cleanup without leaking the previous agent state', async () => {
    const user = userEvent.setup();
    localStorage.setItem(
      'animaos.checkins.agent-1',
      JSON.stringify([
        {
          id: 'checkin-1',
          prompt: 'Reset my goals',
          intervalSecs: 60,
          createdAtMs: 0,
        },
      ]),
    );
    const reset = deferred<Awaited<ReturnType<typeof daemon.deleteAgent>>>();
    vi.spyOn(daemon, 'listAgents')
      .mockResolvedValueOnce({ agents: [snapshot()] })
      .mockResolvedValue({ agents: [] });
    vi.spyOn(daemon, 'listProviders').mockResolvedValue({
      providers: configuredProviders,
    });
    vi.spyOn(daemon, 'deleteAgent').mockReturnValue(reset.promise);
    vi.spyOn(daemon, 'createAgent').mockResolvedValue({
      agent: namedSnapshot('Second', 'agent-2'),
    });
    render(<ViewHarness />);
    expect(
      await screen.findByRole('heading', { name: 'Say something to Nova' }),
    ).toBeVisible();
    await user.click(screen.getByRole('button', { name: /^Activity/ }));
    expect(await screen.findByText('Reset my goals')).toBeVisible();
    await user.click(screen.getAllByRole('button', { name: 'Settings' })[0]);
    await user.click(screen.getByRole('button', { name: 'Reset' }));
    await waitFor(() => expect(daemon.deleteAgent).toHaveBeenCalledTimes(1));

    await act(async () => {
      reset.resolve({ deleted: true });
      await reset.promise;
    });
    expect(localStorage.getItem('animaos.checkins.agent-1')).toBeNull();
    expect(
      await screen.findByRole('heading', { name: 'Create your main agent' }),
    ).toBeVisible();

    await goToReview(user);
    await user.click(screen.getByRole('button', { name: 'Create agent' }));
    expect(
      await screen.findByRole('heading', { name: 'Say something to Second' }),
    ).toBeVisible();
  });

  it('releases the due check-in lock from its snapshot while a slow poll continues', async () => {
    const now = 100_000;
    vi.spyOn(Date, 'now').mockReturnValue(now);
    localStorage.setItem(
      'animaos.checkins.agent-1',
      JSON.stringify([
        {
          id: 'checkin-1',
          prompt: 'Check my goals',
          intervalSecs: 1,
          createdAtMs: 0,
        },
      ]),
    );
    const slowPoll = deferred<{ agents: DaemonSnapshot[] }>();
    const listAgents = vi
      .spyOn(daemon, 'listAgents')
      .mockResolvedValueOnce({ agents: [snapshot()] })
      .mockReturnValue(slowPoll.promise);
    vi.spyOn(daemon, 'listProviders').mockResolvedValue({
      providers: configuredProviders,
    });
    const runAgent = vi.spyOn(daemon, 'runAgent').mockResolvedValue({
      agent: snapshotWithReply('Focus on Task 5'),
      result: {
        status: 'success',
        durationMs: 1,
        data: { text: 'Focus on Task 5' },
      },
    });
    let runPoll: (() => void) | undefined;
    let runCheckins: (() => unknown) | undefined;
    vi.spyOn(window, 'setTimeout').mockImplementation(((
      handler: TimerHandler,
      timeout?: number,
    ) => {
      if (typeof handler === 'function' && timeout === 5_000) {
        runPoll = handler;
        return 1;
      }
      return nativeSetTimeout(handler, timeout);
    }) as typeof window.setTimeout);
    vi.spyOn(window, 'setInterval').mockImplementation(((
      handler: TimerHandler,
      timeout?: number,
    ) => {
      if (typeof handler === 'function' && timeout === 10_000) {
        runCheckins = handler;
      }
      return 1;
    }) as typeof window.setInterval);

    render(<ViewHarness />);
    expect(
      await screen.findByRole('heading', { name: 'Say something to Nova' }),
    ).toBeVisible();
    await userEvent.click(screen.getByRole('button', { name: /^Activity/ }));
    expect(await screen.findByText('Check my goals')).toBeVisible();

    act(() => {
      runPoll?.();
    });
    expect(listAgents).toHaveBeenCalledTimes(2);

    act(() => {
      void runCheckins?.();
    });
    await waitFor(() => expect(runAgent).toHaveBeenCalledTimes(1));
    expect(await screen.findByText(/sent a message/)).toBeVisible();

    vi.mocked(Date.now).mockReturnValue(now + 2_000);
    act(() => {
      void runCheckins?.();
    });
    await waitFor(() => expect(runAgent).toHaveBeenCalledTimes(2));
    expect(listAgents).toHaveBeenCalledTimes(2);
  });

  it('keeps a newer settings snapshot when an older check-in resolves', async () => {
    const user = userEvent.setup();
    const now = 100_000;
    vi.spyOn(Date, 'now').mockReturnValue(now);
    localStorage.setItem(
      'animaos.checkins.agent-1',
      JSON.stringify([
        {
          id: 'checkin-1',
          prompt: 'Check my goals',
          intervalSecs: 1,
          createdAtMs: 0,
        },
      ]),
    );
    const pendingCheckin =
      deferred<Awaited<ReturnType<typeof daemon.runAgent>>>();
    vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [snapshot()] });
    vi.spyOn(daemon, 'listProviders').mockResolvedValue({
      providers: configuredProviders,
    });
    vi.spyOn(daemon, 'runAgent').mockReturnValue(pendingCheckin.promise);
    vi.spyOn(daemon, 'updateAgent').mockResolvedValue({
      agent: namedSnapshot('Newest Settings', 'agent-1'),
    });
    let runCheckins: (() => unknown) | undefined;
    vi.spyOn(window, 'setInterval').mockImplementation(((
      handler: TimerHandler,
      timeout?: number,
    ) => {
      if (typeof handler === 'function' && timeout === 10_000) {
        runCheckins = handler;
      }
      return 1;
    }) as typeof window.setInterval);

    render(<ViewHarness />);
    expect(
      await screen.findByRole('heading', { name: 'Say something to Nova' }),
    ).toBeVisible();
    await user.click(screen.getByRole('button', { name: /^Activity/ }));
    expect(await screen.findByText('Check my goals')).toBeVisible();

    act(() => {
      void runCheckins?.();
    });
    await waitFor(() => expect(daemon.runAgent).toHaveBeenCalledTimes(1));

    await user.click(screen.getAllByRole('button', { name: 'Settings' })[0]);
    const name = screen.getByDisplayValue('Nova');
    await user.clear(name);
    await user.type(name, 'Newest Settings');
    await user.click(screen.getByRole('button', { name: 'Save changes' }));
    expect(await screen.findByDisplayValue('Newest Settings')).toBeVisible();

    await act(async () => {
      pendingCheckin.resolve({
        agent: namedSnapshot('Older Check-in', 'agent-1'),
        result: {
          status: 'success',
          durationMs: 1,
          data: { text: 'Focus on Task 5' },
        },
      });
      await pendingCheckin.promise;
    });

    expect(screen.getByDisplayValue('Newest Settings')).toBeVisible();
    expect(
      screen.queryByDisplayValue('Older Check-in'),
    ).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Close settings' }));
    expect(await screen.findByText(/sent a message/)).toBeVisible();
  });

  it('does not stamp or unlock newer agent check-ins when a reset check-in resolves', async () => {
    const user = userEvent.setup();
    const now = 100_000;
    vi.spyOn(Date, 'now').mockReturnValue(now);
    localStorage.setItem(
      'animaos.checkins.agent-1',
      JSON.stringify([
        {
          id: 'shared-checkin',
          prompt: 'Deleted agent goals',
          intervalSecs: 1,
          createdAtMs: 0,
        },
      ]),
    );
    localStorage.setItem(
      'animaos.checkins.agent-2',
      JSON.stringify([
        {
          id: 'shared-checkin',
          prompt: 'Current agent goals',
          intervalSecs: 1,
          createdAtMs: 0,
        },
      ]),
    );
    const olderCheckin =
      deferred<Awaited<ReturnType<typeof daemon.runAgent>>>();
    const newerCheckin =
      deferred<Awaited<ReturnType<typeof daemon.runAgent>>>();
    vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [snapshot()] });
    vi.spyOn(daemon, 'listProviders').mockResolvedValue({
      providers: configuredProviders,
    });
    const runAgent = vi
      .spyOn(daemon, 'runAgent')
      .mockReturnValueOnce(olderCheckin.promise)
      .mockReturnValueOnce(newerCheckin.promise)
      .mockResolvedValue({
        agent: namedSnapshot('Second', 'agent-2'),
        result: {
          status: 'success',
          durationMs: 1,
          data: { text: CHECKIN_SENTINEL },
        },
      });
    vi.spyOn(daemon, 'deleteAgent').mockResolvedValue({ deleted: true });
    vi.spyOn(daemon, 'createAgent').mockResolvedValue({
      agent: namedSnapshot('Second', 'agent-2'),
    });
    let runCheckins: (() => unknown) | undefined;
    vi.spyOn(window, 'setInterval').mockImplementation(((
      handler: TimerHandler,
      timeout?: number,
    ) => {
      if (typeof handler === 'function' && timeout === 10_000) {
        runCheckins = handler;
      }
      return 1;
    }) as typeof window.setInterval);

    render(<ViewHarness />);
    expect(
      await screen.findByRole('heading', { name: 'Say something to Nova' }),
    ).toBeVisible();
    await user.click(screen.getByRole('button', { name: /^Activity/ }));
    expect(await screen.findByText('Deleted agent goals')).toBeVisible();
    act(() => {
      void runCheckins?.();
    });
    await waitFor(() => expect(runAgent).toHaveBeenCalledTimes(1));

    await user.click(screen.getAllByRole('button', { name: 'Settings' })[0]);
    await user.click(screen.getByRole('button', { name: 'Reset' }));
    expect(
      await screen.findByRole('heading', { name: 'Create your main agent' }),
    ).toBeVisible();
    expect(localStorage.getItem('animaos.checkins.agent-1')).toBeNull();

    await goToReview(user);
    await user.click(screen.getByRole('button', { name: 'Create agent' }));
    expect(
      await screen.findByRole('heading', { name: 'Say something to Second' }),
    ).toBeVisible();
    await user.click(screen.getByRole('button', { name: /^Activity/ }));
    expect(await screen.findByText('Current agent goals')).toBeVisible();
    expect(screen.getByText('has not run yet')).toBeVisible();

    act(() => {
      void runCheckins?.();
    });
    await waitFor(() => expect(runAgent).toHaveBeenCalledTimes(2));

    await act(async () => {
      olderCheckin.resolve({
        agent: namedSnapshot('Nova', 'agent-1'),
        result: {
          status: 'success',
          durationMs: 1,
          data: { text: 'Stale reply' },
        },
      });
      await olderCheckin.promise;
    });

    expect(localStorage.getItem('animaos.checkins.agent-1')).toBeNull();
    expect(screen.getByText('Current agent goals')).toBeVisible();
    expect(screen.getByText('has not run yet')).toBeVisible();
    expect(screen.queryByText('Stale reply')).not.toBeInTheDocument();

    vi.mocked(Date.now).mockReturnValue(now + 2_000);
    act(() => {
      void runCheckins?.();
    });
    expect(runAgent).toHaveBeenCalledTimes(2);

    await act(async () => {
      newerCheckin.resolve({
        agent: namedSnapshot('Second', 'agent-2'),
        result: {
          status: 'success',
          durationMs: 1,
          data: { text: CHECKIN_SENTINEL },
        },
      });
      await newerCheckin.promise;
    });
    expect(await screen.findByText(/stayed silent/)).toBeVisible();

    vi.mocked(Date.now).mockReturnValue(now + 4_000);
    act(() => {
      void runCheckins?.();
    });
    await waitFor(() => expect(runAgent).toHaveBeenCalledTimes(3));
    expect(localStorage.getItem('animaos.checkins.agent-1')).toBeNull();
  });

  it('stops a captured due queue before an old check-in can invalidate new agent work', async () => {
    const user = userEvent.setup();
    vi.spyOn(Date, 'now').mockReturnValue(100_000);
    localStorage.setItem(
      'animaos.checkins.agent-1',
      JSON.stringify([
        {
          id: 'old-checkin-1',
          prompt: 'First old check-in',
          intervalSecs: 1,
          createdAtMs: 0,
        },
        {
          id: 'old-checkin-2',
          prompt: 'Second old check-in',
          intervalSecs: 1,
          createdAtMs: 0,
        },
      ]),
    );
    const firstOldCheckin =
      deferred<Awaited<ReturnType<typeof daemon.runAgent>>>();
    const newAgentRun = deferred<Awaited<ReturnType<typeof daemon.runAgent>>>();
    vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [snapshot()] });
    vi.spyOn(daemon, 'listProviders').mockResolvedValue({
      providers: configuredProviders,
    });
    const runAgent = vi
      .spyOn(daemon, 'runAgent')
      .mockReturnValueOnce(firstOldCheckin.promise)
      .mockReturnValueOnce(newAgentRun.promise)
      .mockResolvedValue({
        agent: snapshot(),
        result: {
          status: 'success',
          durationMs: 1,
          data: { text: CHECKIN_SENTINEL },
        },
      });
    vi.spyOn(daemon, 'deleteAgent').mockResolvedValue({ deleted: true });
    vi.spyOn(daemon, 'createAgent').mockResolvedValue({
      agent: namedSnapshot('Second', 'agent-2'),
    });
    let runCheckins: (() => Promise<void>) | undefined;
    vi.spyOn(window, 'setInterval').mockImplementation(((
      handler: TimerHandler,
      timeout?: number,
    ) => {
      if (typeof handler === 'function' && timeout === 10_000) {
        runCheckins = handler as () => Promise<void>;
      }
      return 1;
    }) as typeof window.setInterval);

    render(<ViewHarness />);
    expect(
      await screen.findByRole('heading', { name: 'Say something to Nova' }),
    ).toBeVisible();
    await user.click(screen.getByRole('button', { name: /^Activity/ }));
    expect(await screen.findByText('First old check-in')).toBeVisible();
    expect(screen.getByText('Second old check-in')).toBeVisible();

    let oldQueue: Promise<void> | undefined;
    act(() => {
      oldQueue = runCheckins?.();
    });
    await waitFor(() => expect(runAgent).toHaveBeenCalledTimes(1));

    await user.click(screen.getAllByRole('button', { name: 'Settings' })[0]);
    await user.click(screen.getByRole('button', { name: 'Reset' }));
    expect(
      await screen.findByRole('heading', { name: 'Create your main agent' }),
    ).toBeVisible();
    await goToReview(user);
    await user.click(screen.getByRole('button', { name: 'Create agent' }));
    expect(
      await screen.findByRole('heading', { name: 'Say something to Second' }),
    ).toBeVisible();

    await user.type(screen.getByPlaceholderText('Message Second…'), 'New work');
    await user.click(screen.getByRole('button', { name: 'Send' }));
    await waitFor(() => expect(runAgent).toHaveBeenCalledTimes(2));

    await act(async () => {
      firstOldCheckin.resolve({
        agent: snapshot(),
        result: {
          status: 'success',
          durationMs: 1,
          data: { text: CHECKIN_SENTINEL },
        },
      });
      await oldQueue;
    });

    expect(runAgent).toHaveBeenCalledTimes(2);
    expect(runAgent).not.toHaveBeenCalledWith(
      'agent-1',
      expect.stringContaining('Second old check-in'),
      expect.anything(),
    );

    await act(async () => {
      newAgentRun.resolve({
        agent: namedSnapshotWithReply('Second', 'agent-2', 'Current reply'),
        result: {
          status: 'success',
          durationMs: 1,
          data: { text: 'Current reply' },
        },
      });
      await newAgentRun.promise;
    });
    expect(await screen.findByText('Current reply')).toBeVisible();
  });

  it('stops a captured due queue when settings starts between check-ins', async () => {
    const user = userEvent.setup();
    vi.spyOn(Date, 'now').mockReturnValue(100_000);
    localStorage.setItem(
      'animaos.checkins.agent-1',
      JSON.stringify([
        {
          id: 'checkin-1',
          prompt: 'First check-in before settings',
          intervalSecs: 1,
          createdAtMs: 0,
        },
        {
          id: 'checkin-2',
          prompt: 'Second check-in blocked by settings',
          intervalSecs: 1,
          createdAtMs: 0,
        },
      ]),
    );
    const firstCheckin =
      deferred<Awaited<ReturnType<typeof daemon.runAgent>>>();
    const settingsUpdate =
      deferred<Awaited<ReturnType<typeof daemon.updateAgent>>>();
    vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [snapshot()] });
    vi.spyOn(daemon, 'listProviders').mockResolvedValue({
      providers: configuredProviders,
    });
    const runAgent = vi
      .spyOn(daemon, 'runAgent')
      .mockReturnValueOnce(firstCheckin.promise)
      .mockResolvedValue({
        agent: snapshot(),
        result: {
          status: 'success',
          durationMs: 1,
          data: { text: CHECKIN_SENTINEL },
        },
      });
    vi.spyOn(daemon, 'updateAgent').mockReturnValue(settingsUpdate.promise);
    let runCheckins: (() => Promise<void>) | undefined;
    vi.spyOn(window, 'setInterval').mockImplementation(((
      handler: TimerHandler,
      timeout?: number,
    ) => {
      if (typeof handler === 'function' && timeout === 10_000) {
        runCheckins = handler as () => Promise<void>;
      }
      return 1;
    }) as typeof window.setInterval);

    render(<ViewHarness />);
    expect(
      await screen.findByRole('heading', { name: 'Say something to Nova' }),
    ).toBeVisible();
    await user.click(screen.getByRole('button', { name: /^Activity/ }));
    expect(
      await screen.findByText('First check-in before settings'),
    ).toBeVisible();

    let queue: Promise<void> | undefined;
    act(() => {
      queue = runCheckins?.();
    });
    await waitFor(() => expect(runAgent).toHaveBeenCalledTimes(1));

    await user.click(screen.getAllByRole('button', { name: 'Settings' })[0]);
    const name = screen.getByDisplayValue('Nova');
    await user.clear(name);
    await user.type(name, 'Saved foreground');
    await user.click(screen.getByRole('button', { name: 'Save changes' }));
    await waitFor(() => expect(daemon.updateAgent).toHaveBeenCalledTimes(1));
    expect(screen.getByRole('button', { name: 'Saving…' })).toBeDisabled();

    await act(async () => {
      firstCheckin.resolve({
        agent: snapshot(),
        result: {
          status: 'success',
          durationMs: 1,
          data: { text: CHECKIN_SENTINEL },
        },
      });
      await queue;
    });

    expect(runAgent).toHaveBeenCalledTimes(1);
    expect(runAgent).not.toHaveBeenCalledWith(
      'agent-1',
      expect.stringContaining('Second check-in blocked by settings'),
      expect.anything(),
    );
    expect(screen.getByRole('button', { name: 'Saving…' })).toBeDisabled();

    await act(async () => {
      settingsUpdate.resolve({
        agent: namedSnapshot('Saved foreground', 'agent-1'),
      });
      await settingsUpdate.promise;
    });
    expect(
      await screen.findByRole('button', { name: 'Saved ✓' }),
    ).toBeVisible();
  });

  it('does not let a same-turn check-in supersede a send that just started', async () => {
    const user = userEvent.setup();
    vi.spyOn(Date, 'now').mockReturnValue(100_000);
    localStorage.setItem(
      'animaos.checkins.agent-1',
      JSON.stringify([
        {
          id: 'checkin-1',
          prompt: 'Do not supersede foreground send',
          intervalSecs: 1,
          createdAtMs: 0,
        },
      ]),
    );
    const sendRun = deferred<Awaited<ReturnType<typeof daemon.runAgent>>>();
    vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [snapshot()] });
    vi.spyOn(daemon, 'listProviders').mockResolvedValue({
      providers: configuredProviders,
    });
    const runAgent = vi
      .spyOn(daemon, 'runAgent')
      .mockReturnValueOnce(sendRun.promise)
      .mockResolvedValue({
        agent: snapshot(),
        result: {
          status: 'success',
          durationMs: 1,
          data: { text: CHECKIN_SENTINEL },
        },
      });
    let runCheckins: (() => unknown) | undefined;
    vi.spyOn(window, 'setInterval').mockImplementation(((
      handler: TimerHandler,
      timeout?: number,
    ) => {
      if (typeof handler === 'function' && timeout === 10_000) {
        runCheckins = handler;
      }
      return 1;
    }) as typeof window.setInterval);

    render(<ViewHarness />);
    expect(
      await screen.findByRole('heading', { name: 'Say something to Nova' }),
    ).toBeVisible();
    await user.type(
      screen.getByPlaceholderText('Message Nova…'),
      'Foreground work',
    );

    act(() => {
      fireEvent.click(screen.getByRole('button', { name: 'Send' }));
      void runCheckins?.();
    });
    expect(runAgent).toHaveBeenCalledTimes(1);
    expect(runAgent).toHaveBeenCalledWith('agent-1', 'Foreground work');

    await act(async () => {
      sendRun.resolve({
        agent: snapshotWithReply('Foreground reply'),
        result: {
          status: 'success',
          durationMs: 1,
          data: { text: 'Foreground reply' },
        },
      });
      await sendRun.promise;
    });
    expect(await screen.findByText('Foreground reply')).toBeVisible();

    act(() => {
      void runCheckins?.();
    });
    await waitFor(() => expect(runAgent).toHaveBeenCalledTimes(2));
  });

  it('stops a captured due queue when a send starts between check-ins', async () => {
    const user = userEvent.setup();
    vi.spyOn(Date, 'now').mockReturnValue(100_000);
    localStorage.setItem(
      'animaos.checkins.agent-1',
      JSON.stringify([
        {
          id: 'checkin-1',
          prompt: 'First check-in before send',
          intervalSecs: 1,
          createdAtMs: 0,
        },
        {
          id: 'checkin-2',
          prompt: 'Second check-in blocked by send',
          intervalSecs: 1,
          createdAtMs: 0,
        },
      ]),
    );
    const firstCheckin =
      deferred<Awaited<ReturnType<typeof daemon.runAgent>>>();
    const sendRun = deferred<Awaited<ReturnType<typeof daemon.runAgent>>>();
    vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [snapshot()] });
    vi.spyOn(daemon, 'listProviders').mockResolvedValue({
      providers: configuredProviders,
    });
    const runAgent = vi
      .spyOn(daemon, 'runAgent')
      .mockReturnValueOnce(firstCheckin.promise)
      .mockReturnValueOnce(sendRun.promise)
      .mockResolvedValue({
        agent: snapshot(),
        result: {
          status: 'success',
          durationMs: 1,
          data: { text: CHECKIN_SENTINEL },
        },
      });
    let runCheckins: (() => Promise<void>) | undefined;
    vi.spyOn(window, 'setInterval').mockImplementation(((
      handler: TimerHandler,
      timeout?: number,
    ) => {
      if (typeof handler === 'function' && timeout === 10_000) {
        runCheckins = handler as () => Promise<void>;
      }
      return 1;
    }) as typeof window.setInterval);

    render(<ViewHarness />);
    expect(
      await screen.findByRole('heading', { name: 'Say something to Nova' }),
    ).toBeVisible();
    await user.click(screen.getByRole('button', { name: /^Activity/ }));
    expect(await screen.findByText('First check-in before send')).toBeVisible();

    let queue: Promise<void> | undefined;
    act(() => {
      queue = runCheckins?.();
    });
    await waitFor(() => expect(runAgent).toHaveBeenCalledTimes(1));

    await user.click(screen.getByRole('button', { name: 'Workspace' }));
    await user.type(
      screen.getByPlaceholderText('Message Nova…'),
      'Foreground work',
    );
    await user.click(screen.getByRole('button', { name: 'Send' }));
    await waitFor(() => expect(runAgent).toHaveBeenCalledTimes(2));
    expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled();

    await act(async () => {
      firstCheckin.resolve({
        agent: snapshot(),
        result: {
          status: 'success',
          durationMs: 1,
          data: { text: CHECKIN_SENTINEL },
        },
      });
      await queue;
    });

    expect(runAgent).toHaveBeenCalledTimes(2);
    expect(runAgent).not.toHaveBeenCalledWith(
      'agent-1',
      expect.stringContaining('Second check-in blocked by send'),
      expect.anything(),
    );
    expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled();

    await act(async () => {
      sendRun.resolve({
        agent: snapshotWithReply('Foreground reply'),
        result: {
          status: 'success',
          durationMs: 1,
          data: { text: 'Foreground reply' },
        },
      });
      await sendRun.promise;
    });
    expect(await screen.findByText('Foreground reply')).toBeVisible();
  });

  it('stops an active due queue as soon as reset owns the agent lifecycle', async () => {
    const user = userEvent.setup();
    vi.spyOn(Date, 'now').mockReturnValue(100_000);
    localStorage.setItem(
      'animaos.checkins.agent-1',
      JSON.stringify([
        {
          id: 'checkin-1',
          prompt: 'First pending check-in',
          intervalSecs: 1,
          createdAtMs: 0,
        },
        {
          id: 'checkin-2',
          prompt: 'Second blocked check-in',
          intervalSecs: 1,
          createdAtMs: 0,
        },
      ]),
    );
    const firstCheckin =
      deferred<Awaited<ReturnType<typeof daemon.runAgent>>>();
    const reset = deferred<Awaited<ReturnType<typeof daemon.deleteAgent>>>();
    vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [snapshot()] });
    vi.spyOn(daemon, 'listProviders').mockResolvedValue({
      providers: configuredProviders,
    });
    const runAgent = vi
      .spyOn(daemon, 'runAgent')
      .mockReturnValueOnce(firstCheckin.promise)
      .mockResolvedValue({
        agent: snapshot(),
        result: {
          status: 'success',
          durationMs: 1,
          data: { text: CHECKIN_SENTINEL },
        },
      });
    vi.spyOn(daemon, 'deleteAgent').mockReturnValue(reset.promise);
    let runCheckins: (() => Promise<void>) | undefined;
    vi.spyOn(window, 'setInterval').mockImplementation(((
      handler: TimerHandler,
      timeout?: number,
    ) => {
      if (typeof handler === 'function' && timeout === 10_000) {
        runCheckins = handler as () => Promise<void>;
      }
      return 1;
    }) as typeof window.setInterval);

    render(<ViewHarness />);
    expect(
      await screen.findByRole('heading', { name: 'Say something to Nova' }),
    ).toBeVisible();
    await user.click(screen.getByRole('button', { name: /^Activity/ }));

    let oldQueue: Promise<void> | undefined;
    act(() => {
      oldQueue = runCheckins?.();
    });
    await waitFor(() => expect(runAgent).toHaveBeenCalledTimes(1));

    await user.click(screen.getAllByRole('button', { name: 'Settings' })[0]);
    await user.click(screen.getByRole('button', { name: 'Reset' }));
    await waitFor(() => expect(daemon.deleteAgent).toHaveBeenCalledTimes(1));

    await act(async () => {
      firstCheckin.resolve({
        agent: snapshot(),
        result: {
          status: 'success',
          durationMs: 1,
          data: { text: CHECKIN_SENTINEL },
        },
      });
      await oldQueue;
    });

    expect(runAgent).toHaveBeenCalledTimes(1);
    expect(runAgent).not.toHaveBeenCalledWith(
      'agent-1',
      expect.stringContaining('Second blocked check-in'),
      expect.anything(),
    );

    await act(async () => {
      reset.reject(new Error('reset failed'));
      await reset.promise.catch(() => undefined);
    });
    expect((await screen.findAllByText('reset failed')).length).toBeGreaterThan(
      0,
    );
  });

  it('does not start a check-in while a failing reset owns the agent lifecycle', async () => {
    const user = userEvent.setup();
    vi.spyOn(Date, 'now').mockReturnValue(100_000);
    localStorage.setItem(
      'animaos.checkins.agent-1',
      JSON.stringify([
        {
          id: 'checkin-1',
          prompt: 'Do not run during reset',
          intervalSecs: 1,
          createdAtMs: 0,
        },
      ]),
    );
    const reset = deferred<Awaited<ReturnType<typeof daemon.deleteAgent>>>();
    vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [snapshot()] });
    vi.spyOn(daemon, 'listProviders').mockResolvedValue({
      providers: configuredProviders,
    });
    const runAgent = vi.spyOn(daemon, 'runAgent').mockResolvedValue({
      agent: snapshot(),
      result: {
        status: 'success',
        durationMs: 1,
        data: { text: CHECKIN_SENTINEL },
      },
    });
    vi.spyOn(daemon, 'deleteAgent').mockReturnValue(reset.promise);
    let runCheckins: (() => unknown) | undefined;
    vi.spyOn(window, 'setInterval').mockImplementation(((
      handler: TimerHandler,
      timeout?: number,
    ) => {
      if (typeof handler === 'function' && timeout === 10_000) {
        runCheckins = handler;
      }
      return 1;
    }) as typeof window.setInterval);

    render(<ViewHarness />);
    expect(
      await screen.findByRole('heading', { name: 'Say something to Nova' }),
    ).toBeVisible();
    await user.click(screen.getAllByRole('button', { name: 'Settings' })[0]);
    await user.click(screen.getByRole('button', { name: 'Reset' }));
    await waitFor(() => expect(daemon.deleteAgent).toHaveBeenCalledTimes(1));

    await act(async () => {
      await runCheckins?.();
    });
    expect(runAgent).not.toHaveBeenCalled();

    await act(async () => {
      reset.reject(new Error('reset failed'));
      await reset.promise.catch(() => undefined);
    });

    expect((await screen.findAllByText('reset failed')).length).toBeGreaterThan(
      0,
    );
    await user.click(screen.getByRole('button', { name: 'Close settings' }));
    expect(
      screen.getByRole('heading', { name: 'Say something to Nova' }),
    ).toBeVisible();

    act(() => {
      void runCheckins?.();
    });
    await waitFor(() => expect(runAgent).toHaveBeenCalledTimes(1));
  });

  it('keeps a deferred reset exclusive and reports its failure on the preserved agent', async () => {
    const user = userEvent.setup();
    vi.spyOn(Date, 'now').mockReturnValue(100_000);
    localStorage.setItem(
      'animaos.checkins.agent-1',
      JSON.stringify([
        {
          id: 'checkin-1',
          prompt: 'Blocked during reset',
          intervalSecs: 1,
          createdAtMs: 0,
        },
      ]),
    );
    const reset = deferred<Awaited<ReturnType<typeof daemon.deleteAgent>>>();
    vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [snapshot()] });
    vi.spyOn(daemon, 'listProviders').mockResolvedValue({
      providers: configuredProviders,
    });
    const deleteAgent = vi
      .spyOn(daemon, 'deleteAgent')
      .mockReturnValue(reset.promise);
    const updateAgent = vi.spyOn(daemon, 'updateAgent').mockResolvedValue({
      agent: namedSnapshot('Changed during reset', 'agent-1'),
    });
    const runAgent = vi.spyOn(daemon, 'runAgent').mockResolvedValue({
      agent: snapshotWithReply('Should not run'),
      result: {
        status: 'success',
        durationMs: 1,
        data: { text: 'Should not run' },
      },
    });
    let runCheckins: (() => unknown) | undefined;
    vi.spyOn(window, 'setInterval').mockImplementation(((
      handler: TimerHandler,
      timeout?: number,
    ) => {
      if (typeof handler === 'function' && timeout === 10_000) {
        runCheckins = handler;
      }
      return 1;
    }) as typeof window.setInterval);

    render(<ViewHarness />);
    expect(
      await screen.findByRole('heading', { name: 'Say something to Nova' }),
    ).toBeVisible();
    await user.click(screen.getAllByRole('button', { name: 'Settings' })[0]);
    const name = screen.getByDisplayValue('Nova');
    await user.clear(name);
    await user.type(name, 'Changed during reset');
    await user.click(screen.getByRole('button', { name: 'Reset' }));
    await waitFor(() => expect(deleteAgent).toHaveBeenCalledTimes(1));

    await user.click(screen.getByRole('button', { name: 'Resetting…' }));
    await user.click(screen.getByRole('button', { name: 'Save changes' }));
    act(() => {
      void runCheckins?.();
    });
    await user.click(screen.getByRole('button', { name: 'Close settings' }));
    expect(
      screen.getByRole('heading', { name: 'Agent settings' }),
    ).toBeVisible();
    await user.type(screen.getByPlaceholderText(/Message/), 'Blocked send');
    fireEvent.click(screen.getByRole('button', { name: 'Send', hidden: true }));

    await act(async () => {
      reset.reject(new Error('exclusive reset failed'));
      await reset.promise.catch(() => undefined);
    });

    expect(deleteAgent).toHaveBeenCalledTimes(1);
    expect(updateAgent).not.toHaveBeenCalled();
    expect(runAgent).not.toHaveBeenCalled();
    expect(
      await screen.findByRole('heading', {
        name: 'Say something to Nova',
        hidden: true,
      }),
    ).toBeVisible();
    expect(await screen.findByText('exclusive reset failed')).toBeVisible();
    expect(screen.getByRole('alert')).toHaveFocus();
    await user.click(screen.getByRole('button', { name: 'Close settings' }));
    expect(
      screen.queryByText('exclusive reset failed'),
    ).not.toBeInTheDocument();
  });

  it('disables and announces reset controls and the composer while delete is pending', async () => {
    const user = userEvent.setup();
    const reset = deferred<Awaited<ReturnType<typeof daemon.deleteAgent>>>();
    vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [snapshot()] });
    vi.spyOn(daemon, 'listProviders').mockResolvedValue({
      providers: configuredProviders,
    });
    vi.spyOn(daemon, 'deleteAgent').mockReturnValue(reset.promise);

    render(<ViewHarness />);
    expect(
      await screen.findByRole('heading', { name: 'Say something to Nova' }),
    ).toBeVisible();
    const composer = screen.getByPlaceholderText('Message Nova…');
    await user.type(composer, 'Keep this draft');
    await user.click(screen.getAllByRole('button', { name: 'Settings' })[0]);
    const name = screen.getByDisplayValue('Nova');
    await user.clear(name);
    await user.type(name, 'Changed name');
    const save = screen.getByRole('button', { name: 'Save changes' });

    await user.click(screen.getByRole('button', { name: 'Reset' }));
    await waitFor(() => expect(daemon.deleteAgent).toHaveBeenCalledTimes(1));

    expect(save).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Resetting…' })).toBeDisabled();
    expect(screen.getByRole('status')).toHaveTextContent('Resetting agent…');

    await user.click(screen.getByRole('button', { name: 'Close settings' }));
    expect(composer).toBeDisabled();
    expect(
      screen.getByRole('button', { name: 'Send', hidden: true }),
    ).toBeDisabled();

    await act(async () => {
      reset.reject(new Error('reset interrupted'));
      await reset.promise.catch(() => undefined);
    });

    expect(composer).toBeEnabled();
    expect(
      screen.getByRole('button', { name: 'Send', hidden: true }),
    ).toBeEnabled();
  });

  it('keeps the newest provider response when Strict Mode requests finish in reverse order', async () => {
    const user = userEvent.setup();
    const olderProviders = deferred<{ providers: DaemonProvider[] }>();
    const newerProviders = deferred<{ providers: DaemonProvider[] }>();
    vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [] });
    const listProviders = vi
      .spyOn(daemon, 'listProviders')
      .mockReturnValueOnce(olderProviders.promise)
      .mockReturnValueOnce(newerProviders.promise);

    render(
      <StrictMode>
        <ViewHarness />
      </StrictMode>,
    );
    expect(
      await screen.findByRole('heading', { name: 'Create your main agent' }),
    ).toBeVisible();
    await waitFor(() => expect(listProviders).toHaveBeenCalledTimes(2));
    await goToIntelligence(user);

    await act(async () => {
      newerProviders.resolve({ providers: configuredProviders });
      await newerProviders.promise;
    });
    expect(
      await screen.findByRole('button', { name: /OpenAI.*configured/i }),
    ).toBeVisible();

    await act(async () => {
      olderProviders.reject(new Error('stale provider failure'));
      await olderProviders.promise.catch(() => undefined);
    });
    expect(
      screen.getByRole('button', { name: /OpenAI.*configured/i }),
    ).toBeVisible();
    expect(
      screen.queryByText('stale provider failure'),
    ).not.toBeInTheDocument();
  });

  it('keeps a polled existing agent when a deferred onboarding create resolves later', async () => {
    const user = userEvent.setup();
    const existing = namedSnapshot('Existing', 'agent-existing');
    const created = namedSnapshot('Late create', 'agent-created');
    const create = deferred<{ agent: DaemonSnapshot }>();
    const listAgents = vi
      .spyOn(daemon, 'listAgents')
      .mockResolvedValueOnce({ agents: [] })
      .mockResolvedValue({ agents: [existing] });
    vi.spyOn(daemon, 'listProviders').mockResolvedValue({
      providers: configuredProviders,
    });
    vi.spyOn(daemon, 'createAgent').mockReturnValue(create.promise);
    let runPoll: (() => void) | undefined;
    vi.spyOn(window, 'setTimeout').mockImplementation(((
      handler: TimerHandler,
      timeout?: number,
    ) => {
      if (typeof handler === 'function' && timeout === 5_000) {
        runPoll = handler;
        return 1;
      }
      return nativeSetTimeout(handler, timeout);
    }) as typeof window.setTimeout);

    render(<ViewHarness />);
    expect(
      await screen.findByRole('heading', { name: 'Create your main agent' }),
    ).toBeVisible();
    await goToReview(user);
    await user.click(screen.getByRole('button', { name: 'Create agent' }));
    await waitFor(() => expect(daemon.createAgent).toHaveBeenCalledTimes(1));

    act(() => {
      runPoll?.();
    });
    expect(
      await screen.findByRole('heading', { name: 'Say something to Existing' }),
    ).toBeVisible();
    expect(listAgents).toHaveBeenCalledTimes(2);

    await act(async () => {
      create.resolve({ agent: created });
      await create.promise;
    });

    expect(
      screen.getByRole('heading', { name: 'Say something to Existing' }),
    ).toBeVisible();
    expect(
      screen.queryByRole('heading', { name: 'Say something to Late create' }),
    ).not.toBeInTheDocument();
  });

  it('lets ViewHarness adopt the created snapshot without another agent-list request', async () => {
    const user = userEvent.setup();
    const created = snapshot();
    const stalePoll = deferred<{ agents: DaemonSnapshot[] }>();
    const listAgents = vi
      .spyOn(daemon, 'listAgents')
      .mockResolvedValueOnce({ agents: [] })
      .mockReturnValueOnce(stalePoll.promise);
    vi.spyOn(daemon, 'listProviders').mockResolvedValue({
      providers: configuredProviders,
    });
    vi.spyOn(daemon, 'createAgent').mockResolvedValue({ agent: created });
    let runPoll: (() => void) | undefined;
    vi.spyOn(window, 'setTimeout').mockImplementation(((
      handler: TimerHandler,
      timeout?: number,
    ) => {
      if (typeof handler === 'function' && timeout === 5_000) {
        runPoll = handler;
        return 1;
      }
      return nativeSetTimeout(handler, timeout);
    }) as typeof window.setTimeout);

    render(<ViewHarness />);
    expect(
      await screen.findByRole('heading', { name: 'Create your main agent' }),
    ).toBeVisible();
    act(() => {
      runPoll?.();
    });
    await waitFor(() => expect(listAgents).toHaveBeenCalledTimes(2));
    const listCallsBeforeCreate = listAgents.mock.calls.length;

    await goToReview(user);
    await user.click(screen.getByRole('button', { name: 'Create agent' }));

    expect(
      await screen.findByRole('heading', { name: 'Say something to Nova' }),
    ).toBeVisible();
    expect(listAgents).toHaveBeenCalledTimes(listCallsBeforeCreate);

    await act(async () => {
      stalePoll.resolve({ agents: [] });
      await stalePoll.promise;
    });
    expect(
      screen.getByRole('heading', { name: 'Say something to Nova' }),
    ).toBeVisible();
    expect(
      screen.queryByRole('heading', { name: 'Create your main agent' }),
    ).not.toBeInTheDocument();
    expect(screen.getByText('Daemon Online')).toBeVisible();
  });
});
