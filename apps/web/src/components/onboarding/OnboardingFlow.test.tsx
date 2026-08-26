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
    const retryProviders = vi.fn();
    const view = renderFlow({
      providers: null,
      providersError: null,
      retryProviders,
    });

    const name = screen.getByRole('textbox', { name: 'Agent name' });
    await user.clear(name);
    await user.type(name, 'Persistent Anima');
    await goToIntelligence(user);
    expect(screen.getByText('Loading provider catalog…')).toBeVisible();
    expect(screen.getByLabelText('Provider catalog')).toHaveAttribute(
      'aria-busy',
      'true',
    );

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
    await user.click(screen.getByRole('button', { name: 'Retry providers' }));
    expect(retryProviders).toHaveBeenCalledTimes(1);

    view.rerender(
      <OnboardingFlow
        providers={configuredProviders}
        providersError={null}
        retryProviders={retryProviders}
        onCreated={vi.fn()}
      />,
    );
    await user.click(screen.getByRole('button', { name: 'Back' }));
    expect(screen.getByRole('textbox', { name: 'Agent name' })).toHaveValue(
      'Persistent Anima',
    );
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

  it('keeps a rejected creation on Review with an alert and intact retryable choices', async () => {
    const user = userEvent.setup();
    const createAgent = vi
      .spyOn(daemon, 'createAgent')
      .mockRejectedValueOnce(new Error('daemon refused creation'))
      .mockResolvedValueOnce({ agent: snapshot() });
    const onCreated = vi.fn();
    renderFlow({ onCreated });

    await goToReview(user);
    await user.click(screen.getByRole('button', { name: 'Create agent' }));

    expect(await screen.findByRole('alert')).toHaveTextContent(
      'daemon refused creation',
    );
    expect(screen.getByRole('heading', { name: 'Review' })).toBeVisible();
    expect(screen.getByText('Anima')).toBeVisible();
    expect(screen.getByText('OpenAI / gpt-4o')).toBeVisible();
    expect(screen.getByText('Collaborate')).toBeVisible();

    await user.click(screen.getByRole('button', { name: 'Create agent' }));
    await waitFor(() => expect(onCreated).toHaveBeenCalledWith(snapshot()));
    expect(createAgent).toHaveBeenCalledTimes(2);
  });

  it('lets ViewHarness adopt the created snapshot without another agent-list request', async () => {
    const user = userEvent.setup();
    const created = snapshot();
    const stalePoll = deferred<{ agents: DaemonSnapshot[] }>();
    const staleFailure = deferred<{ agents: DaemonSnapshot[] }>();
    const listAgents = vi
      .spyOn(daemon, 'listAgents')
      .mockResolvedValueOnce({ agents: [] })
      .mockReturnValueOnce(stalePoll.promise)
      .mockReturnValueOnce(staleFailure.promise);
    vi.spyOn(daemon, 'listProviders').mockResolvedValue({
      providers: configuredProviders,
    });
    vi.spyOn(daemon, 'createAgent').mockResolvedValue({ agent: created });
    vi.spyOn(window, 'setInterval').mockImplementation(((
      handler: TimerHandler,
    ) => {
      if (typeof handler === 'function') {
        queueMicrotask(() => {
          handler();
          handler();
        });
      }
      return 1;
    }) as typeof window.setInterval);

    render(<ViewHarness />);
    expect(
      await screen.findByRole('heading', { name: 'Create your main agent' }),
    ).toBeVisible();
    await waitFor(() => expect(listAgents).toHaveBeenCalledTimes(3));
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
      staleFailure.reject(new Error('stale poll failed'));
      await staleFailure.promise.catch(() => undefined);
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
