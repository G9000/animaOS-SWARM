import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { toolNamesForProfile } from '../../lib/agent-access';
import { presetTemplate } from '../../lib/agent-presets';
import {
  daemon,
  PROFILE_GENERATION_UNAVAILABLE,
  type DaemonProvider,
  type DaemonSnapshot,
  type WorkspaceInspectFound,
} from '../../lib/daemon-api';
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

const WORKSPACE = {
  companyName: 'Acme',
  mission: 'Build calm tools',
  rootPath: '/tmp/acme',
};

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

async function fillWorkspace(user: ReturnType<typeof userEvent.setup>) {
  await user.type(
    screen.getByRole('textbox', { name: 'Company name' }),
    WORKSPACE.companyName,
  );
  await user.type(
    screen.getByRole('textbox', { name: 'Mission (one sentence)' }),
    WORKSPACE.mission,
  );
  const rootPath = screen.getByRole('textbox', { name: 'Office location' });
  await user.clear(rootPath);
  await user.type(rootPath, WORKSPACE.rootPath);
}

async function goToIntelligence(user: ReturnType<typeof userEvent.setup>) {
  await fillWorkspace(user);
  await user.click(screen.getByRole('button', { name: 'Next' }));
  expect(screen.getByRole('heading', { name: 'Intelligence' })).toBeVisible();
}

async function goToAgent(user: ReturnType<typeof userEvent.setup>) {
  await goToIntelligence(user);
  await user.click(screen.getByRole('button', { name: 'Next' }));
  expect(screen.getByRole('heading', { name: 'Agent' })).toBeVisible();
}

async function goToAccess(user: ReturnType<typeof userEvent.setup>) {
  await goToAgent(user);
  await user.click(screen.getByRole('button', { name: 'Next' }));
  expect(screen.getByRole('heading', { name: 'Access' })).toBeVisible();
}

async function goToReview(user: ReturnType<typeof userEvent.setup>) {
  await goToAccess(user);
  await user.click(screen.getByRole('button', { name: 'Next' }));
  expect(screen.getByRole('heading', { name: 'Review' })).toBeVisible();
}

beforeEach(() => {
  vi.spyOn(daemon, 'getWorkspace').mockResolvedValue({
    configured: false,
    workspace: null,
    defaultRoot: '',
  });
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('OnboardingFlow', () => {
  it('renders the Workspace step first and pre-fills the daemon default root', async () => {
    vi.spyOn(daemon, 'getWorkspace').mockResolvedValue({
      configured: false,
      workspace: null,
      defaultRoot: '/Users/dev/anima',
    });
    renderFlow();

    expect(screen.getByRole('heading', { name: 'Workspace' })).toBeVisible();
    expect(
      screen.getByRole('heading', { name: 'Set up your workspace' }),
    ).toBeVisible();
    const company = screen.getByRole('textbox', { name: 'Company name' });
    expect(company).toHaveValue('');
    expect(company).toBeRequired();
    expect(company).toHaveAttribute('aria-invalid', 'false');
    expect(screen.getAllByRole('listitem')).toHaveLength(5);
    expect(screen.getAllByRole('listitem')[0]).toHaveAttribute(
      'aria-current',
      'step',
    );
    expect(screen.getByRole('status')).toHaveTextContent(
      'Step 1 of 5: Workspace',
    );

    expect(await screen.findByDisplayValue('/Users/dev/anima')).toBeVisible();
  });

  it('blocks Next with an empty workspace draft and focuses the company input', async () => {
    const user = userEvent.setup();
    renderFlow();

    await user.click(screen.getByRole('button', { name: 'Next' }));

    expect(screen.getByRole('heading', { name: 'Workspace' })).toBeVisible();
    const alert = screen.getByRole('alert');
    expect(alert).toHaveTextContent(
      'Enter a company name, mission, and workspace folder.',
    );
    expect(alert).toHaveAttribute('id', 'onboarding-workspace-error');
    const company = screen.getByRole('textbox', { name: 'Company name' });
    expect(company).toHaveAttribute('aria-invalid', 'true');
    expect(company).toHaveAttribute(
      'aria-describedby',
      'onboarding-workspace-error',
    );
    expect(company).toHaveFocus();
    expect(screen.getAllByRole('listitem')[0]).toHaveAttribute(
      'aria-current',
      'step',
    );

    await fillWorkspace(user);
    expect(company).toHaveAttribute('aria-invalid', 'false');
    expect(company).not.toHaveAttribute('aria-describedby');
    await user.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByRole('heading', { name: 'Intelligence' })).toBeVisible();
  });

  it('reports verify failure inline, keeps the draft, reports willCreate, and resets on edits', async () => {
    const user = userEvent.setup();
    const validateWorkspace = vi
      .spyOn(daemon, 'validateWorkspace')
      .mockRejectedValueOnce(new Error('folder not writable'))
      .mockResolvedValue({
        configured: false,
        workspace: null,
        defaultRoot: '',
        rootPathExists: false,
      });
    renderFlow();
    await fillWorkspace(user);

    await user.click(screen.getByRole('button', { name: 'Verify' }));
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'folder not writable',
    );
    expect(validateWorkspace).toHaveBeenCalledWith({
      rootPath: WORKSPACE.rootPath,
      companyName: WORKSPACE.companyName,
      mission: WORKSPACE.mission,
      values: [],
    });
    expect(screen.getByRole('textbox', { name: 'Company name' })).toHaveValue(
      WORKSPACE.companyName,
    );
    expect(
      screen.getByRole('textbox', { name: 'Mission (one sentence)' }),
    ).toHaveValue(WORKSPACE.mission);
    expect(
      screen.getByRole('textbox', { name: 'Office location' }),
    ).toHaveValue(WORKSPACE.rootPath);

    await user.click(screen.getByRole('button', { name: 'Verify' }));
    expect(await screen.findByText(/Folder will be created/)).toBeVisible();

    await user.type(
      screen.getByRole('textbox', { name: 'Office location' }),
      '2',
    );
    expect(
      screen.queryByText(/Folder will be created/),
    ).not.toBeInTheDocument();
  });

  it('focuses the first empty workspace field on a blocking error', async () => {
    const user = userEvent.setup();
    renderFlow();

    await user.type(
      screen.getByRole('textbox', { name: 'Company name' }),
      WORKSPACE.companyName,
    );
    await user.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByRole('alert')).toHaveTextContent(
      'Enter a company name, mission, and workspace folder.',
    );
    expect(
      screen.getByRole('textbox', { name: 'Mission (one sentence)' }),
    ).toHaveFocus();

    await user.type(
      screen.getByRole('textbox', { name: 'Mission (one sentence)' }),
      WORKSPACE.mission,
    );
    await user.click(screen.getByRole('button', { name: 'Next' }));
    expect(
      screen.getByRole('textbox', { name: 'Office location' }),
    ).toHaveFocus();
  });

  it('ignores a stale verify response after the root path changes', async () => {
    const user = userEvent.setup();
    const pending =
      deferred<Awaited<ReturnType<typeof daemon.validateWorkspace>>>();
    vi.spyOn(daemon, 'validateWorkspace').mockReturnValue(pending.promise);
    renderFlow();
    await fillWorkspace(user);

    await user.click(screen.getByRole('button', { name: 'Verify' }));
    await user.type(
      screen.getByRole('textbox', { name: 'Office location' }),
      'x',
    );

    await act(async () => {
      pending.resolve({
        configured: false,
        workspace: null,
        defaultRoot: '',
        rootPathExists: false,
      });
      await pending.promise;
    });

    expect(
      screen.queryByText(/Folder will be created/),
    ).not.toBeInTheDocument();
    expect(screen.queryByText(/Folder exists/)).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Verify' })).toBeEnabled();
  });

  it('keeps edits typed during generation and only fills still-untouched fields', async () => {
    const user = userEvent.setup();
    const pending =
      deferred<Awaited<ReturnType<typeof daemon.generateProfile>>>();
    vi.spyOn(daemon, 'generateProfile').mockReturnValue(pending.promise);
    renderFlow();
    await goToAgent(user);

    await user.type(
      screen.getByRole('textbox', { name: /What do you want/ }),
      'Run my ops',
    );
    await user.click(screen.getByRole('button', { name: /generate profile/i }));
    await user.type(screen.getByRole('textbox', { name: 'Bio' }), 'My own bio');

    await act(async () => {
      pending.resolve({
        profile: {
          bio: 'Generated bio',
          adjectives: ['crisp'],
          style: 'Generated style',
          system: 'Generated system',
        },
      });
      await pending.promise;
    });

    expect(screen.getByRole('textbox', { name: 'Bio' })).toHaveValue(
      'My own bio',
    );
    expect(screen.getByRole('textbox', { name: 'Style' })).toHaveValue(
      'Generated style',
    );
    expect(screen.getByRole('textbox', { name: 'Instructions' })).toHaveValue(
      'Generated system',
    );
    expect(screen.getByText('crisp')).toBeVisible();
  });

  it('walks Workspace → Intelligence → Agent → Access → Review and Back preserves every draft', async () => {
    const user = userEvent.setup();
    renderFlow();

    expect(screen.getAllByRole('listitem')).toHaveLength(5);
    expect(screen.getByRole('status')).toHaveTextContent(
      'Step 1 of 5: Workspace',
    );

    await fillWorkspace(user);
    await user.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByRole('status')).toHaveTextContent(
      'Step 2 of 5: Intelligence',
    );

    await user.selectOptions(
      screen.getByRole('combobox', { name: 'Model' }),
      'gpt-4.1',
    );
    await user.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByRole('status')).toHaveTextContent('Step 3 of 5: Agent');

    const name = screen.getByRole('textbox', { name: 'Name' });
    await user.clear(name);
    await user.type(name, 'Nova');
    await user.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByRole('status')).toHaveTextContent('Step 4 of 5: Access');

    await user.click(screen.getByRole('radio', { name: /Operate/ }));
    await user.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByRole('heading', { name: 'Review' })).toBeVisible();
    expect(screen.getAllByRole('listitem')[4]).toHaveAttribute(
      'aria-current',
      'step',
    );
    expect(screen.getByRole('status')).toHaveTextContent('Step 5 of 5: Review');

    await user.click(screen.getByRole('button', { name: 'Back' }));
    expect(screen.getByRole('radio', { name: /Operate/ })).toBeChecked();
    await user.click(screen.getByRole('button', { name: 'Back' }));
    expect(screen.getByRole('textbox', { name: 'Name' })).toHaveValue('Nova');
    const template = presetTemplate('chief-of-staff', {
      companyName: WORKSPACE.companyName,
      mission: WORKSPACE.mission,
      agentName: 'Nova',
    });
    expect(screen.getByRole('textbox', { name: 'Instructions' })).toHaveValue(
      template.system,
    );
    await user.click(screen.getByRole('button', { name: 'Back' }));
    expect(screen.getByRole('combobox', { name: 'Model' })).toHaveValue(
      'gpt-4.1',
    );
    await user.click(screen.getByRole('button', { name: 'Back' }));
    expect(screen.getByRole('textbox', { name: 'Company name' })).toHaveValue(
      WORKSPACE.companyName,
    );
    expect(
      screen.getByRole('textbox', { name: 'Mission (one sentence)' }),
    ).toHaveValue(WORKSPACE.mission);
    expect(
      screen.getByRole('textbox', { name: 'Office location' }),
    ).toHaveValue(WORKSPACE.rootPath);
    expect(screen.getAllByRole('listitem')[0]).toHaveAttribute(
      'aria-current',
      'step',
    );
  });

  it('blocks only the Intelligence step on a provider catalog error and preserves the workspace draft', async () => {
    const user = userEvent.setup();
    const retryProviders = vi.fn();
    renderFlow({
      providers: null,
      providersError: 'provider catalog failed',
      retryProviders,
    });

    await fillWorkspace(user);
    await user.click(screen.getByRole('button', { name: 'Next' }));

    expect(screen.getByRole('heading', { name: 'Intelligence' })).toBeVisible();
    expect(screen.getByRole('alert')).toHaveTextContent(
      'provider catalog failed',
    );
    expect(screen.getByRole('button', { name: 'Next' })).toBeDisabled();
    expect(
      screen.getByRole('button', { name: 'Retry providers' }),
    ).toHaveFocus();

    await user.click(screen.getByRole('button', { name: 'Back' }));
    expect(screen.getByRole('textbox', { name: 'Company name' })).toHaveValue(
      WORKSPACE.companyName,
    );
    expect(
      screen.getByRole('textbox', { name: 'Mission (one sentence)' }),
    ).toHaveValue(WORKSPACE.mission);
    expect(
      screen.getByRole('textbox', { name: 'Office location' }),
    ).toHaveValue(WORKSPACE.rootPath);
  });

  it('fills the bio, style, and instructions from the preset template when picking a preset', async () => {
    const user = userEvent.setup();
    renderFlow();
    await goToAgent(user);

    expect(screen.getByRole('textbox', { name: 'Bio' })).toHaveValue('');
    expect(screen.getByRole('textbox', { name: 'Instructions' })).toHaveValue(
      '',
    );

    await user.click(screen.getByRole('radio', { name: /Senior Engineer/ }));

    const template = presetTemplate('senior-engineer', {
      companyName: WORKSPACE.companyName,
      mission: WORKSPACE.mission,
      agentName: 'Anima',
    });
    expect(screen.getByRole('textbox', { name: 'Bio' })).toHaveValue(
      template.bio,
    );
    expect(screen.getByRole('textbox', { name: 'Style' })).toHaveValue(
      template.style,
    );
    expect(screen.getByRole('textbox', { name: 'Instructions' })).toHaveValue(
      template.system,
    );
    expect(screen.getByText('direct')).toBeVisible();
    expect(screen.getByText('precise')).toBeVisible();
    expect(screen.getByText('pragmatic')).toBeVisible();
  });

  it('generates a profile through the daemon with preset, intent, model, and workspace context', async () => {
    const user = userEvent.setup();
    const profile = {
      bio: 'Generated bio',
      adjectives: ['crisp', 'calm'],
      style: 'Generated style',
      system: 'Generated system',
    };
    const generateProfile = vi
      .spyOn(daemon, 'generateProfile')
      .mockResolvedValue({ profile });
    renderFlow();
    await goToAgent(user);

    await user.click(screen.getByRole('radio', { name: /Senior Engineer/ }));
    await user.type(
      screen.getByRole('textbox', { name: /What do you want/ }),
      'Run my ops',
    );
    await user.click(screen.getByRole('button', { name: /generate profile/i }));

    await waitFor(() =>
      expect(screen.getByRole('textbox', { name: 'Bio' })).toHaveValue(
        'Generated bio',
      ),
    );
    expect(generateProfile).toHaveBeenCalledTimes(1);
    expect(generateProfile).toHaveBeenCalledWith({
      presetId: 'senior-engineer',
      intent: 'Run my ops',
      provider: 'openai',
      model: 'gpt-4o',
      workspace: {
        companyName: WORKSPACE.companyName,
        mission: WORKSPACE.mission,
        values: [],
      },
    });
    expect(screen.getByRole('textbox', { name: 'Style' })).toHaveValue(
      'Generated style',
    );
    expect(screen.getByRole('textbox', { name: 'Instructions' })).toHaveValue(
      'Generated system',
    );
    expect(screen.getByText('crisp')).toBeVisible();
    expect(screen.getByText('calm')).toBeVisible();
  });

  it('falls back to the preset template with a notice when profile generation is unavailable', async () => {
    const user = userEvent.setup();
    vi.spyOn(daemon, 'generateProfile').mockRejectedValue(
      new Error(`${PROFILE_GENERATION_UNAVAILABLE}: no generative provider`),
    );
    renderFlow();
    await goToAgent(user);

    await user.type(
      screen.getByRole('textbox', { name: /What do you want/ }),
      'Run my ops',
    );
    await user.click(screen.getByRole('button', { name: /generate profile/i }));

    expect(
      await screen.findByText(/No generative provider configured/),
    ).toBeVisible();
    const template = presetTemplate('chief-of-staff', {
      companyName: WORKSPACE.companyName,
      mission: WORKSPACE.mission,
      agentName: 'Anima',
    });
    expect(screen.getByRole('textbox', { name: 'Bio' })).toHaveValue(
      template.bio,
    );
    expect(screen.getByRole('textbox', { name: 'Style' })).toHaveValue(
      template.style,
    );
    expect(screen.getByRole('textbox', { name: 'Instructions' })).toHaveValue(
      template.system,
    );
    expect(
      screen.getByRole('button', { name: /generate profile/i }),
    ).toBeDisabled();
  });

  it('surfaces other generation errors inline and keeps the entered profile fields', async () => {
    const user = userEvent.setup();
    vi.spyOn(daemon, 'generateProfile').mockRejectedValue(
      new Error('model timed out'),
    );
    renderFlow();
    await goToAgent(user);

    await user.type(screen.getByRole('textbox', { name: 'Bio' }), 'My own bio');
    await user.type(
      screen.getByRole('textbox', { name: /What do you want/ }),
      'Run my ops',
    );
    await user.click(screen.getByRole('button', { name: /generate profile/i }));

    expect(await screen.findByRole('alert')).toHaveTextContent(
      'model timed out',
    );
    expect(screen.getByRole('textbox', { name: 'Bio' })).toHaveValue(
      'My own bio',
    );
    expect(
      screen.queryByText(/No generative provider configured/),
    ).not.toBeInTheDocument();
  });

  it('summarizes the workspace, preset, agent name, provider/model, and access on Review', async () => {
    const user = userEvent.setup();
    renderFlow();
    await goToAgent(user);

    const name = screen.getByRole('textbox', { name: 'Name' });
    await user.clear(name);
    await user.type(name, 'Nova');
    await user.click(screen.getByRole('button', { name: 'Next' }));
    await user.click(screen.getByRole('radio', { name: /Operate/ }));
    await user.click(screen.getByRole('button', { name: 'Next' }));

    expect(screen.getByText(WORKSPACE.companyName)).toBeVisible();
    expect(screen.getByText(WORKSPACE.mission)).toBeVisible();
    const rootPath = screen.getByTitle(WORKSPACE.rootPath);
    expect(rootPath).toHaveTextContent(WORKSPACE.rootPath);
    expect(screen.getByText('Chief of Staff')).toBeVisible();

    const template = presetTemplate('chief-of-staff', {
      companyName: WORKSPACE.companyName,
      mission: WORKSPACE.mission,
      agentName: 'Nova',
    });
    expect(screen.getByText(template.bio)).toBeVisible();

    expect(screen.getByText('Nova')).toBeVisible();
    expect(screen.getByText('OpenAI / gpt-4o')).toBeVisible();
    expect(screen.getByText('Operate')).toBeVisible();
    expect(
      screen.getByText(
        'Can execute shell commands and manage background processes.',
      ),
    ).toBeVisible();
    expect(
      screen.getByText(/if anything fails, nothing is half-created/),
    ).toBeVisible();
  });

  it('bootstraps once with the full workspace and agent payload and hands off the snapshot', async () => {
    const user = userEvent.setup();
    const created = snapshot();
    const pending =
      deferred<Awaited<ReturnType<typeof daemon.bootstrapWorkspace>>>();
    const bootstrapWorkspace = vi
      .spyOn(daemon, 'bootstrapWorkspace')
      .mockReturnValue(pending.promise);
    const onCreated = vi.fn();
    renderFlow({ onCreated });
    await goToAgent(user);

    const name = screen.getByRole('textbox', { name: 'Name' });
    await user.clear(name);
    await user.type(name, '  Nova  ');
    await user.click(screen.getByRole('button', { name: 'Next' }));
    await user.click(screen.getByRole('radio', { name: /Operate/ }));
    await user.click(screen.getByRole('button', { name: 'Next' }));
    await user.click(screen.getByRole('button', { name: 'Create agent' }));

    const creatingButton = await screen.findByRole('button', {
      name: 'Creating agent…',
    });
    expect(creatingButton).toBeDisabled();
    await user.click(creatingButton);
    expect(bootstrapWorkspace).toHaveBeenCalledTimes(1);

    const template = presetTemplate('chief-of-staff', {
      companyName: WORKSPACE.companyName,
      mission: WORKSPACE.mission,
      agentName: 'Nova',
    });
    expect(bootstrapWorkspace).toHaveBeenCalledWith({
      workspace: {
        rootPath: WORKSPACE.rootPath,
        companyName: WORKSPACE.companyName,
        mission: WORKSPACE.mission,
        values: [],
      },
      agent: {
        name: 'Nova',
        presetId: 'chief-of-staff',
        bio: template.bio,
        adjectives: template.adjectives,
        style: template.style,
        system: template.system,
        provider: 'openai',
        model: 'gpt-4o',
        tools: toolNamesForProfile('operate'),
      },
    });

    await act(async () => {
      pending.resolve({
        workspace: {
          rootPath: WORKSPACE.rootPath,
          companyName: WORKSPACE.companyName,
          mission: WORKSPACE.mission,
          values: [],
        },
        agent: created,
      });
      await pending.promise;
    });
    await waitFor(() => expect(onCreated).toHaveBeenCalledWith(created));
    expect(bootstrapWorkspace).toHaveBeenCalledTimes(1);
  });

  it('re-uses createAgent and prefills the draft when the workspace is already configured', async () => {
    const user = userEvent.setup();
    const existingWorkspace = {
      rootPath: '/srv/acme',
      companyName: 'Acme Corp',
      mission: 'Ship calmly',
      values: ['cite sources'],
    };
    vi.spyOn(daemon, 'getWorkspace').mockResolvedValue({
      configured: true,
      workspace: existingWorkspace,
      defaultRoot: '/Users/dev/anima',
    });
    const created = snapshot();
    const createAgent = vi
      .spyOn(daemon, 'createAgent')
      .mockResolvedValue({ agent: created });
    const bootstrapWorkspace = vi.spyOn(daemon, 'bootstrapWorkspace');
    const onCreated = vi.fn();
    renderFlow({ onCreated });

    expect(await screen.findByDisplayValue('Acme Corp')).toBeVisible();
    expect(
      screen.getByRole('textbox', { name: 'Mission (one sentence)' }),
    ).toHaveValue('Ship calmly');
    expect(
      screen.getByRole('textbox', { name: 'Office location' }),
    ).toHaveValue('/srv/acme');
    expect(screen.getByRole('textbox', { name: /Values/ })).toHaveValue(
      'cite sources',
    );
    expect(
      screen.getByText('Your workspace is ready — hire another agent.'),
    ).toBeVisible();

    await user.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByRole('heading', { name: 'Intelligence' })).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Next' }));
    const name = screen.getByRole('textbox', { name: 'Name' });
    await user.clear(name);
    await user.type(name, 'Nova');
    await user.click(screen.getByRole('button', { name: 'Next' }));
    await user.click(screen.getByRole('radio', { name: /Operate/ }));
    await user.click(screen.getByRole('button', { name: 'Next' }));

    expect(screen.getByRole('heading', { name: 'Review' })).toBeVisible();
    expect(screen.queryByText(/anima\.yaml/)).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Create agent' }));

    await waitFor(() => expect(onCreated).toHaveBeenCalledWith(created));
    const template = presetTemplate('chief-of-staff', {
      companyName: 'Acme Corp',
      mission: 'Ship calmly',
      agentName: 'Nova',
    });
    expect(createAgent).toHaveBeenCalledTimes(1);
    expect(createAgent).toHaveBeenCalledWith({
      name: 'Nova',
      provider: 'openai',
      model: 'gpt-4o',
      system: template.system,
      tools: toolNamesForProfile('operate'),
    });
    expect(bootstrapWorkspace).not.toHaveBeenCalled();
  });

  it('stays on Review with the full draft intact when bootstrap fails', async () => {
    const user = userEvent.setup();
    const bootstrapWorkspace = vi
      .spyOn(daemon, 'bootstrapWorkspace')
      .mockRejectedValue(new Error('bootstrap refused'));
    const onCreated = vi.fn();
    renderFlow({ onCreated });
    await goToAgent(user);

    const name = screen.getByRole('textbox', { name: 'Name' });
    await user.clear(name);
    await user.type(name, 'Nova');
    await user.click(screen.getByRole('button', { name: 'Next' }));
    await user.click(screen.getByRole('radio', { name: /Operate/ }));
    await user.click(screen.getByRole('button', { name: 'Next' }));
    await user.click(screen.getByRole('button', { name: 'Create agent' }));

    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('bootstrap refused');
    expect(alert).toHaveFocus();
    expect(screen.getByRole('heading', { name: 'Review' })).toBeVisible();
    expect(screen.getByText('Nova')).toBeVisible();
    expect(screen.getByText('OpenAI / gpt-4o')).toBeVisible();
    expect(screen.getByText('Operate')).toBeVisible();
    expect(onCreated).not.toHaveBeenCalled();

    await user.click(screen.getByRole('button', { name: 'Back' }));
    expect(screen.getByRole('radio', { name: /Operate/ })).toBeChecked();
    await user.click(screen.getByRole('button', { name: 'Back' }));
    expect(screen.getByRole('textbox', { name: 'Name' })).toHaveValue('Nova');
    await user.click(screen.getByRole('button', { name: 'Back' }));
    expect(screen.getByRole('combobox', { name: 'Model' })).toHaveValue(
      'gpt-4o',
    );
    await user.click(screen.getByRole('button', { name: 'Back' }));
    expect(screen.getByRole('textbox', { name: 'Company name' })).toHaveValue(
      WORKSPACE.companyName,
    );
    expect(bootstrapWorkspace).toHaveBeenCalledTimes(1);
  });

  it('blocks an empty custom model at Intelligence and keeps the draft on retry', async () => {
    const user = userEvent.setup();
    renderFlow();
    await goToIntelligence(user);

    await user.selectOptions(
      screen.getByRole('combobox', { name: 'Model' }),
      '__custom__',
    );
    const customModel = screen.getByRole('textbox', { name: 'Custom model' });
    await user.type(customModel, '   ');
    await user.click(screen.getByRole('button', { name: 'Next' }));

    expect(screen.getByRole('heading', { name: 'Intelligence' })).toBeVisible();
    const alert = screen.getByRole('alert');
    expect(alert).toHaveTextContent('Enter a model.');
    expect(alert).toHaveAttribute('id', 'onboarding-custom-model-error');
    expect(customModel).toHaveAttribute('aria-invalid', 'true');
    expect(customModel).toHaveAttribute(
      'aria-describedby',
      'onboarding-custom-model-error',
    );
    expect(customModel).toHaveFocus();

    await user.type(customModel, 'custom/great-model');
    expect(customModel).toHaveAttribute('aria-invalid', 'false');
    await user.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByRole('heading', { name: 'Agent' })).toBeVisible();

    await user.click(screen.getByRole('button', { name: 'Back' }));
    expect(screen.getByRole('combobox', { name: 'Model' })).toHaveValue(
      '__custom__',
    );
    expect(screen.getByRole('textbox', { name: 'Custom model' })).toHaveValue(
      '   custom/great-model',
    );
  });

  it('returns Review to Intelligence when the reviewed provider is invalidated', async () => {
    const user = userEvent.setup();
    const bootstrapWorkspace = vi.spyOn(daemon, 'bootstrapWorkspace');
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
    expect(bootstrapWorkspace).not.toHaveBeenCalled();
  });
});

describe('OnboardingFlow workspace resume', () => {
  const RESUME_ROOT = '/tmp/northwind';

  function inspectPreview(): WorkspaceInspectFound {
    return {
      found: true,
      companyName: 'Northwind Research',
      mission: 'Continuous equity research',
      values: ['cite sources'],
      orchestrator: {
        name: 'Anima',
        bio: 'A vigilant chief of staff.',
        provider: 'moonshot',
        model: 'kimi-k2',
      },
      workers: [
        { name: 'Scout', provider: 'moonshot', model: 'kimi-k2' },
        { name: 'Scribe', provider: 'moonshot', model: 'kimi-k2' },
      ],
      providerAvailable: true,
    };
  }

  async function enterResumeMode(
    user: ReturnType<typeof userEvent.setup>,
    rootPath: string = RESUME_ROOT,
  ) {
    await user.click(
      screen.getByRole('button', { name: /already have a workspace/i }),
    );
    await user.type(
      screen.getByRole('textbox', { name: 'Office location' }),
      rootPath,
    );
  }

  async function showResumeCard(user: ReturnType<typeof userEvent.setup>) {
    await enterResumeMode(user);
    await user.click(screen.getByRole('button', { name: 'Inspect' }));
    expect(
      await screen.findByRole('heading', {
        level: 2,
        name: 'Resume your workspace',
      }),
    ).toBeVisible();
  }

  it('resume mode keeps only the folder field and an Inspect button', async () => {
    const user = userEvent.setup();
    renderFlow();

    await user.click(
      screen.getByRole('button', { name: /already have a workspace/i }),
    );

    expect(
      screen.queryByRole('textbox', { name: 'Company name' }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole('textbox', { name: 'Mission (one sentence)' }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole('textbox', { name: /Values/ }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole('textbox', { name: 'Office location' }),
    ).toBeVisible();
    expect(screen.getByRole('button', { name: 'Inspect' })).toBeVisible();
  });

  it('browses and inspects an existing workspace without creating one', async () => {
    const user = userEvent.setup();
    vi.spyOn(daemon, 'pickWorkspaceFolder').mockResolvedValue({
      rootPath: RESUME_ROOT,
    });
    const inspect = vi
      .spyOn(daemon, 'inspectWorkspace')
      .mockResolvedValue(inspectPreview());
    const { onCreated } = renderFlow();
    await user.click(
      screen.getByRole('button', { name: 'Open existing workspace' }),
    );
    expect(
      screen.queryByRole('list', { name: 'Onboarding progress' }),
    ).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Browse…' }));
    expect(await screen.findByText('Northwind Research')).toBeVisible();
    expect(inspect).toHaveBeenCalledWith(RESUME_ROOT);
    expect(onCreated).not.toHaveBeenCalled();
  });

  it('keeps the path when the folder dialog is cancelled', async () => {
    const user = userEvent.setup();
    vi.spyOn(daemon, 'pickWorkspaceFolder').mockResolvedValue({
      rootPath: null,
    });
    const inspect = vi.spyOn(daemon, 'inspectWorkspace');
    renderFlow();
    await enterResumeMode(user);
    await user.click(screen.getByRole('button', { name: 'Browse…' }));
    expect(
      screen.getByRole('textbox', { name: 'Office location' }),
    ).toHaveValue(RESUME_ROOT);
    expect(inspect).not.toHaveBeenCalled();
  });

  it('shows picker errors and allows retrying', async () => {
    const user = userEvent.setup();
    vi.spyOn(daemon, 'pickWorkspaceFolder').mockRejectedValue(
      new Error('Folder picker unavailable'),
    );
    renderFlow();
    await user.click(screen.getByRole('button', { name: 'Browse…' }));
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'Folder picker unavailable',
    );
    expect(screen.getByRole('button', { name: 'Browse…' })).toBeEnabled();
  });

  it('ignores a folder selection after the user edits the path', async () => {
    const user = userEvent.setup();
    const pending = deferred<{ rootPath: string | null }>();
    vi.spyOn(daemon, 'pickWorkspaceFolder').mockReturnValue(pending.promise);
    renderFlow();
    await user.click(screen.getByRole('button', { name: 'Browse…' }));
    await user.type(
      screen.getByRole('textbox', { name: 'Office location' }),
      '/tmp/new-path',
    );
    await act(async () => pending.resolve({ rootPath: RESUME_ROOT }));
    expect(
      screen.getByRole('textbox', { name: 'Office location' }),
    ).toHaveValue('/tmp/new-path');
  });

  it('ignores a folder selection after leaving resume mode', async () => {
    const user = userEvent.setup();
    const pending = deferred<{ rootPath: string | null }>();
    const picker = vi
      .spyOn(daemon, 'pickWorkspaceFolder')
      .mockReturnValue(pending.promise);
    const inspect = vi.spyOn(daemon, 'inspectWorkspace');
    renderFlow();
    await enterResumeMode(user);
    await user.click(screen.getByRole('button', { name: 'Browse…' }));
    expect(
      screen.getByRole('button', { name: 'Choosing folder…' }),
    ).toBeDisabled();
    await user.click(
      screen.getByRole('button', { name: /set up a new workspace instead/i }),
    );
    await act(async () => pending.resolve({ rootPath: '/tmp/other' }));
    expect(
      screen.getByRole('textbox', { name: 'Office location' }),
    ).toHaveValue(RESUME_ROOT);
    expect(picker).toHaveBeenCalledTimes(1);
    expect(inspect).not.toHaveBeenCalled();
  });

  it('shows an inline note and keeps the wizard unchanged when inspect finds no workspace', async () => {
    const user = userEvent.setup();
    const inspectWorkspace = vi
      .spyOn(daemon, 'inspectWorkspace')
      .mockResolvedValue({ found: false });
    renderFlow();
    await enterResumeMode(user);

    await user.click(screen.getByRole('button', { name: 'Inspect' }));

    expect(await screen.findByText(/no workspace file/i)).toBeVisible();
    expect(inspectWorkspace).toHaveBeenCalledWith(RESUME_ROOT);
    expect(screen.getByRole('heading', { name: 'Workspace' })).toBeVisible();
    expect(
      screen.getByRole('textbox', { name: 'Office location' }),
    ).toHaveValue(RESUME_ROOT);
    expect(
      screen.queryByRole('heading', { name: 'Resume your workspace' }),
    ).not.toBeInTheDocument();
  });

  it('renders the resume card with the preview and hides the step body and nav', async () => {
    const user = userEvent.setup();
    vi.spyOn(daemon, 'inspectWorkspace').mockResolvedValue(inspectPreview());
    renderFlow();

    await showResumeCard(user);

    expect(screen.getByText('Northwind Research')).toBeVisible();
    expect(screen.getByText('Continuous equity research')).toBeVisible();
    expect(screen.getByText('Anima')).toBeVisible();
    expect(screen.getByText('Scout')).toBeVisible();
    expect(screen.getByText('Scribe')).toBeVisible();
    expect(screen.getByTitle(RESUME_ROOT)).toHaveTextContent(RESUME_ROOT);
    expect(
      screen.queryByRole('textbox', { name: 'Office location' }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole('button', { name: 'Next' }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole('button', { name: 'Back' }),
    ).not.toBeInTheDocument();
  });

  it('resumes the workspace once and hands the orchestrator snapshot to onCreated', async () => {
    const user = userEvent.setup();
    vi.spyOn(daemon, 'inspectWorkspace').mockResolvedValue(inspectPreview());
    const created = snapshot();
    const resumeWorkspace = vi
      .spyOn(daemon, 'resumeWorkspace')
      .mockResolvedValue({
        workspace: {
          rootPath: RESUME_ROOT,
          companyName: 'Northwind Research',
          mission: 'Continuous equity research',
          values: ['cite sources'],
        },
        orchestrator: created,
        workers: [],
        skipped: [],
      });
    const onCreated = vi.fn();
    renderFlow({ onCreated });
    await showResumeCard(user);

    await user.click(screen.getByRole('button', { name: 'Resume workspace' }));

    await waitFor(() => expect(onCreated).toHaveBeenCalledWith(created));
    expect(resumeWorkspace).toHaveBeenCalledTimes(1);
    expect(resumeWorkspace).toHaveBeenCalledWith(RESUME_ROOT);
  });

  it('shows a resume error on the card and keeps the preview intact', async () => {
    const user = userEvent.setup();
    vi.spyOn(daemon, 'inspectWorkspace').mockResolvedValue(inspectPreview());
    const resumeWorkspace = vi
      .spyOn(daemon, 'resumeWorkspace')
      .mockRejectedValue(new Error('resume refused'));
    const onCreated = vi.fn();
    renderFlow({ onCreated });
    await showResumeCard(user);

    await user.click(screen.getByRole('button', { name: 'Resume workspace' }));

    expect(await screen.findByRole('alert')).toHaveTextContent(
      'resume refused',
    );
    expect(onCreated).not.toHaveBeenCalled();
    expect(screen.getByText('Northwind Research')).toBeVisible();
    expect(screen.getByText('Scout')).toBeVisible();
    expect(
      screen.getByRole('button', { name: 'Resume workspace' }),
    ).toBeEnabled();
    expect(resumeWorkspace).toHaveBeenCalledTimes(1);
  });

  it('returns to the normal workspace step with the draft intact from the card', async () => {
    const user = userEvent.setup();
    vi.spyOn(daemon, 'inspectWorkspace').mockResolvedValue(inspectPreview());
    renderFlow();
    await showResumeCard(user);

    await user.click(
      screen.getByRole('button', { name: 'Set up fresh instead' }),
    );

    expect(screen.getByRole('heading', { name: 'Workspace' })).toBeVisible();
    expect(screen.getByRole('textbox', { name: 'Company name' })).toBeVisible();
    expect(
      screen.getByRole('textbox', { name: 'Mission (one sentence)' }),
    ).toBeVisible();
    expect(
      screen.getByRole('textbox', { name: 'Office location' }),
    ).toHaveValue(RESUME_ROOT);
    expect(screen.getByRole('button', { name: 'Verify' })).toBeVisible();
    expect(screen.getByRole('button', { name: 'Next' })).toBeVisible();
    expect(
      screen.queryByRole('heading', { name: 'Resume your workspace' }),
    ).not.toBeInTheDocument();
  });

  it('ignores a stale inspect response after the folder path changes', async () => {
    const user = userEvent.setup();
    const pending =
      deferred<Awaited<ReturnType<typeof daemon.inspectWorkspace>>>();
    vi.spyOn(daemon, 'inspectWorkspace').mockReturnValue(pending.promise);
    renderFlow();
    await enterResumeMode(user);

    await user.click(screen.getByRole('button', { name: 'Inspect' }));
    await user.type(
      screen.getByRole('textbox', { name: 'Office location' }),
      'x',
    );

    await act(async () => {
      pending.resolve(inspectPreview());
      await pending.promise;
    });

    expect(
      screen.queryByRole('heading', { name: 'Resume your workspace' }),
    ).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Inspect' })).toBeEnabled();
  });

  it('ignores an inspect response that resolves after leaving resume mode', async () => {
    const user = userEvent.setup();
    const pending =
      deferred<Awaited<ReturnType<typeof daemon.inspectWorkspace>>>();
    vi.spyOn(daemon, 'inspectWorkspace').mockReturnValue(pending.promise);
    renderFlow();
    await enterResumeMode(user);

    await user.click(screen.getByRole('button', { name: 'Inspect' }));
    await user.click(
      screen.getByRole('button', { name: /set up a new workspace instead/i }),
    );

    await act(async () => {
      pending.resolve(inspectPreview());
      await pending.promise;
    });

    expect(
      screen.queryByRole('heading', { name: 'Resume your workspace' }),
    ).not.toBeInTheDocument();
    expect(screen.getByRole('textbox', { name: 'Company name' })).toBeVisible();
    expect(screen.getByRole('button', { name: 'Verify' })).toBeEnabled();
  });

  it('ignores a verify response that resolves after entering resume mode', async () => {
    const user = userEvent.setup();
    const pending =
      deferred<Awaited<ReturnType<typeof daemon.validateWorkspace>>>();
    vi.spyOn(daemon, 'validateWorkspace').mockReturnValue(pending.promise);
    renderFlow();
    await fillWorkspace(user);

    await user.click(screen.getByRole('button', { name: 'Verify' }));
    await user.click(
      screen.getByRole('button', { name: /already have a workspace/i }),
    );

    await act(async () => {
      pending.resolve({
        configured: false,
        workspace: null,
        defaultRoot: '',
        rootPathExists: false,
      });
      await pending.promise;
    });

    expect(
      screen.queryByText(/Folder will be created/),
    ).not.toBeInTheDocument();
    expect(screen.queryByText(/Folder exists/)).not.toBeInTheDocument();
  });

  it('hides Next in resume mode instead of dead-ending on hidden fields', async () => {
    const user = userEvent.setup();
    renderFlow();

    await user.click(
      screen.getByRole('button', { name: /already have a workspace/i }),
    );

    expect(
      screen.queryByRole('button', { name: 'Next' }),
    ).not.toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();

    await user.click(
      screen.getByRole('button', { name: /set up a new workspace instead/i }),
    );
    expect(screen.getByRole('button', { name: 'Next' })).toBeVisible();
  });
});
