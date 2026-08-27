import { StrictMode } from 'react';
import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { toolNamesForProfile } from '../../lib/agent-access';
import {
  daemon,
  type DaemonProvider,
  type DaemonSnapshot,
} from '../../lib/daemon-api';
import { ViewHarness } from '../../ViewHarness';
import { OnboardingFlow } from './OnboardingFlow';

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

    await user.clear(name);
    await user.type(name, '   ');
    await user.type(instructions, 'Keep answers short.');
    await user.click(screen.getByRole('button', { name: 'Next' }));

    expect(screen.getByRole('heading', { name: 'Identity' })).toBeVisible();
    expect(screen.getByRole('alert')).toHaveTextContent('Enter an agent name.');
    expect(name).toHaveFocus();
    expect(screen.getAllByRole('listitem')[0]).toHaveAttribute(
      'aria-current',
      'step',
    );

    await user.clear(name);
    await user.type(name, 'Anima Prime');
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
    expect(openai).toHaveClass('border-sky-400/60', 'bg-sky-400/10');
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
    expect(ollama).toHaveClass('border-sky-400/60', 'bg-sky-400/10');
    expect(openai).not.toHaveClass('border-sky-400/60', 'bg-sky-400/10');
    expect(screen.getByRole('combobox', { name: 'Model' })).toHaveValue(
      'llama3.1',
    );
    expect(screen.getByRole('option', { name: 'qwen2.5' })).toBeVisible();

    await user.selectOptions(
      screen.getByRole('combobox', { name: 'Model' }),
      '__custom__',
    );
    const customModel = screen.getByRole('textbox', { name: 'Custom model' });
    await user.type(customModel, '   ');
    await user.click(screen.getByRole('button', { name: 'Next' }));

    expect(screen.getByRole('heading', { name: 'Intelligence' })).toBeVisible();
    expect(screen.getByRole('alert')).toHaveTextContent('Enter a model.');
    expect(customModel).toHaveFocus();

    await user.type(customModel, 'llama3.2');
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

  it('skips sustained polling ticks while a refresh is active', async () => {
    const initialPoll = deferred<{ agents: DaemonSnapshot[] }>();
    const followupPoll = deferred<{ agents: DaemonSnapshot[] }>();
    const finalPoll = deferred<{ agents: DaemonSnapshot[] }>();
    const listAgents = vi
      .spyOn(daemon, 'listAgents')
      .mockReturnValueOnce(initialPoll.promise)
      .mockReturnValueOnce(followupPoll.promise)
      .mockReturnValueOnce(finalPoll.promise);
    vi.spyOn(daemon, 'listProviders').mockResolvedValue({
      providers: configuredProviders,
    });
    let runPoll: (() => void) | undefined;
    vi.spyOn(window, 'setInterval').mockImplementation(((
      handler: TimerHandler,
      timeout?: number,
    ) => {
      if (typeof handler === 'function' && timeout === 5_000) {
        runPoll = handler;
      }
      return 1;
    }) as typeof window.setInterval);

    render(<ViewHarness />);
    expect(listAgents).toHaveBeenCalledTimes(1);

    act(() => {
      runPoll?.();
      runPoll?.();
      runPoll?.();
    });
    expect(listAgents).toHaveBeenCalledTimes(1);

    await act(async () => {
      initialPoll.resolve({
        agents: [namedSnapshot('Initial', 'agent-initial')],
      });
      await initialPoll.promise;
    });
    expect(
      await screen.findByRole('heading', { name: 'Say something to Initial' }),
    ).toBeVisible();
    expect(listAgents).toHaveBeenCalledTimes(1);

    act(() => {
      runPoll?.();
    });
    expect(listAgents).toHaveBeenCalledTimes(2);

    act(() => {
      runPoll?.();
      runPoll?.();
      runPoll?.();
    });
    expect(listAgents).toHaveBeenCalledTimes(2);

    await act(async () => {
      followupPoll.resolve({
        agents: [namedSnapshot('Followup', 'agent-followup')],
      });
      await followupPoll.promise;
    });
    expect(
      screen.getByRole('heading', { name: 'Say something to Followup' }),
    ).toBeVisible();
    expect(listAgents).toHaveBeenCalledTimes(2);

    act(() => {
      runPoll?.();
    });
    expect(listAgents).toHaveBeenCalledTimes(3);

    await act(async () => {
      finalPoll.resolve({ agents: [namedSnapshot('Final', 'agent-final')] });
      await finalPoll.promise;
    });
    expect(
      screen.getByRole('heading', { name: 'Say something to Final' }),
    ).toBeVisible();
    expect(listAgents).toHaveBeenCalledTimes(3);
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
    vi.spyOn(window, 'setInterval').mockImplementation(((
      handler: TimerHandler,
      timeout?: number,
    ) => {
      if (typeof handler === 'function' && timeout === 5_000) {
        runPoll = handler;
      }
      return 1;
    }) as typeof window.setInterval);

    render(<ViewHarness />);
    expect(
      await screen.findByRole('heading', { name: 'Say something to Nova' }),
    ).toBeVisible();

    act(() => {
      runPoll?.();
      runPoll?.();
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
    vi.spyOn(window, 'setInterval').mockImplementation(((
      handler: TimerHandler,
      timeout?: number,
    ) => {
      if (typeof handler === 'function' && timeout === 5_000) {
        runPoll = handler;
      }
      return 1;
    }) as typeof window.setInterval);

    render(<ViewHarness />);
    expect(
      await screen.findByRole('heading', { name: 'Say something to Nova' }),
    ).toBeVisible();

    act(() => {
      runPoll?.();
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
    vi.spyOn(window, 'setInterval').mockImplementation(((
      handler: TimerHandler,
      timeout?: number,
    ) => {
      if (typeof handler === 'function' && timeout === 5_000) {
        runPoll = handler;
      }
      if (typeof handler === 'function' && timeout === 10_000) {
        runCheckins = handler;
      }
      return 1;
    }) as typeof window.setInterval);

    render(<ViewHarness />);
    expect(
      await screen.findByRole('heading', { name: 'Say something to Nova' }),
    ).toBeVisible();
    await userEvent.click(
      screen.getByRole('button', { name: /^Proactive/ }),
    );
    expect(await screen.findByText('Check my goals')).toBeVisible();

    act(() => {
      runPoll?.();
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

  it('starts a fresh poll after prior request ownership is released', async () => {
    const initialPoll = deferred<{ agents: DaemonSnapshot[] }>();
    const followupPoll = deferred<{ agents: DaemonSnapshot[] }>();
    const listAgents = vi
      .spyOn(daemon, 'listAgents')
      .mockReturnValueOnce(initialPoll.promise)
      .mockReturnValueOnce(followupPoll.promise);
    vi.spyOn(daemon, 'listProviders').mockResolvedValue({
      providers: configuredProviders,
    });
    let runPoll: (() => void) | undefined;
    vi.spyOn(window, 'setInterval').mockImplementation(((
      handler: TimerHandler,
      timeout?: number,
    ) => {
      if (typeof handler === 'function' && timeout === 5_000) {
        runPoll = handler;
      }
      return 1;
    }) as typeof window.setInterval);

    render(<ViewHarness />);
    expect(listAgents).toHaveBeenCalledTimes(1);

    await act(async () => {
      initialPoll.resolve({
        agents: [namedSnapshot('Initial', 'agent-initial')],
      });
      queueMicrotask(() => {
        queueMicrotask(() => runPoll?.());
      });
      await initialPoll.promise;
    });

    expect(
      await screen.findByRole('heading', { name: 'Say something to Initial' }),
    ).toBeVisible();
    await waitFor(() => expect(listAgents).toHaveBeenCalledTimes(2));

    await act(async () => {
      followupPoll.resolve({
        agents: [namedSnapshot('Followup', 'agent-followup')],
      });
      await followupPoll.promise;
    });
    expect(
      screen.getByRole('heading', { name: 'Say something to Followup' }),
    ).toBeVisible();
    expect(listAgents).toHaveBeenCalledTimes(2);
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
    vi.spyOn(window, 'setInterval').mockImplementation(((
      handler: TimerHandler,
      timeout?: number,
    ) => {
      if (typeof handler === 'function' && timeout === 5_000) {
        runPoll = handler;
      }
      return 1;
    }) as typeof window.setInterval);

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
    expect(screen.getByText('daemon connected')).toBeVisible();
  });
});
