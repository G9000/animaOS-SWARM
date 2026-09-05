import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { toolNamesForProfile } from '../../lib/agent-access';
import {
  daemon,
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
    screen.getByRole('textbox', { name: 'Workspace brief' }),
    WORKSPACE.mission,
  );
  const rootPath = screen.getByRole('textbox', { name: 'Office location' });
  await user.clear(rootPath);
  await user.type(rootPath, WORKSPACE.rootPath);
}

async function goToIntelligence(user: ReturnType<typeof userEvent.setup>) {
  await fillWorkspace(user);
  await user.click(screen.getByRole('button', { name: 'Next' }));
  expect(screen.getByRole('heading', { name: 'Model' })).toBeVisible();
}

async function goToManager(user: ReturnType<typeof userEvent.setup>) {
  await goToIntelligence(user);
  await user.click(screen.getByRole('button', { name: 'Next' }));
  expect(
    screen.getByRole('heading', { name: 'Workspace Manager' }),
  ).toBeVisible();
}

async function goToReview(user: ReturnType<typeof userEvent.setup>) {
  await goToManager(user);
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
  it('provides a valid predefined manager without profile generation or editable personality fields', async () => {
    const user = userEvent.setup();
    const generate = vi.spyOn(daemon, 'generateProfile');
    const bootstrap = vi.spyOn(daemon, 'bootstrapWorkspace').mockResolvedValue({
      agent: snapshot(),
      workspace: { ...WORKSPACE, values: [] },
    });
    renderFlow();
    await goToManager(user);
    expect(screen.getByRole('textbox', { name: 'Manager name' })).toHaveValue(
      'Anima',
    );
    expect(screen.getByRole('radio', { name: /^Balanced/ })).toBeChecked();
    expect(screen.getByRole('radio', { name: /^Concise/ })).toBeChecked();
    expect(
      screen.getByRole('textbox', { name: 'Workspace preferences' }),
    ).toHaveValue('');
    for (const name of ['Bio', 'Style', 'Instructions']) {
      expect(
        screen.queryByRole('textbox', { name, exact: true }),
      ).not.toBeInTheDocument();
    }
    expect(
      screen.queryByRole('button', { name: /generate profile/i }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole('group', { name: 'Personality preset' }),
    ).not.toBeInTheDocument();
    await user.click(screen.getByText('View manager instructions'));
    expect(
      screen.getByText(/You are Anima, the workspace manager/),
    ).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Next' }));
    await user.click(screen.getByRole('button', { name: 'Create workspace' }));
    await waitFor(() => expect(bootstrap).toHaveBeenCalledTimes(1));
    const agent = bootstrap.mock.calls[0][0].agent;
    expect(agent.system).toContain('Initiative: balanced');
    expect(agent.system).toContain('Communication: concise');
    expect(agent.system).toContain(WORKSPACE.mission);
    expect(agent.bio).not.toBe('');
    expect(generate).not.toHaveBeenCalled();
  });

  it('persists manager preferences and communication while proactive initiative preserves Observe access', async () => {
    const user = userEvent.setup();
    const bootstrap = vi.spyOn(daemon, 'bootstrapWorkspace').mockResolvedValue({
      agent: snapshot(),
      workspace: { ...WORKSPACE, values: [] },
    });
    renderFlow();
    await goToManager(user);
    await user.click(screen.getByRole('radio', { name: /^Proactive/ }));
    await user.click(screen.getByRole('radio', { name: /^Detailed/ }));
    await user.type(
      screen.getByRole('textbox', { name: 'Workspace preferences' }),
      'Protect focus time and cite sources',
    );
    await user.click(screen.getByRole('radio', { name: /^Observe/ }));
    await user.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByText(/^Proactive$/i)).toBeVisible();
    expect(screen.getByText(/^Detailed$/i)).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Create workspace' }));
    await waitFor(() => expect(bootstrap).toHaveBeenCalledTimes(1));
    const agent = bootstrap.mock.calls[0][0].agent;
    expect(agent.system).toContain('Protect focus time and cite sources');
    expect(agent.system).toContain('Initiative: proactive');
    expect(agent.system).toContain('Communication: detailed');
    expect(agent.system).toContain(agent.style);
    expect(agent.system).toContain(WORKSPACE.mission);
    expect(agent.tools).toEqual(toolNamesForProfile('observe'));
  });

  it('ignores an in-flight generated team after returning to choose another template', async () => {
    const user = userEvent.setup();
    const pending =
      deferred<Awaited<ReturnType<typeof daemon.generateAgency>>>();
    vi.spyOn(daemon, 'generateAgency').mockReturnValue(pending.promise);
    renderFlow();
    await user.click(screen.getByRole('button', { name: /Marketing Agency/ }));
    await user.type(
      screen.getByRole('textbox', { name: 'Office location' }),
      '/tmp/marketing',
    );
    await user.click(screen.getByRole('button', { name: 'Next' }));
    await user.click(screen.getByRole('button', { name: 'Next' }));
    await user.click(screen.getByRole('button', { name: 'Generate team' }));
    expect(
      screen.getByRole('button', { name: 'Edit Strategist' }),
    ).toBeDisabled();
    await user.click(screen.getByRole('button', { name: 'Back' }));
    await user.click(screen.getByRole('button', { name: 'Back' }));
    await user.click(screen.getByRole('button', { name: 'Change template' }));
    await user.click(screen.getByRole('button', { name: /Life Agency/ }));
    await act(async () =>
      pending.resolve({
        name: 'Old',
        agents: [
          { name: 'Stale', role: 'orchestrator', bio: 'Old', system: 'Old' },
        ],
      }),
    );
    await user.click(screen.getByRole('button', { name: 'Next' }));
    await user.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByRole('button', { name: 'Edit Planner' })).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Next' }));
    expect(
      screen.getByRole('textbox', { name: 'Manager name', exact: true }),
    ).toHaveValue('Anima');
  });

  it('generates a preview using the selected model and creates only after review', async () => {
    const user = userEvent.setup();
    const generate = vi.spyOn(daemon, 'generateAgency').mockResolvedValue({
      name: 'Acme',
      agents: [
        {
          name: 'Director',
          role: 'orchestrator',
          bio: 'Direct the team',
          system: 'Lead the work',
        },
        {
          name: 'Editor',
          role: 'worker',
          bio: 'Edit content',
          system: 'Edit carefully',
        },
      ],
    });
    const bootstrap = vi.spyOn(daemon, 'bootstrapWorkspace').mockResolvedValue({
      agent: snapshot(),
      workspace: { ...WORKSPACE, values: [] },
    });
    renderFlow();
    await user.click(
      screen.getByRole('button', { name: /Generate my agency/ }),
    );
    await goToIntelligence(user);
    await user.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByText('Team not generated yet')).toBeVisible();
    await user.selectOptions(
      screen.getByRole('combobox', { name: 'Maximum team size' }),
      '6',
    );
    await user.click(screen.getByRole('button', { name: 'Generate team' }));
    await screen.findByRole('button', { name: 'Edit Editor' });
    await user.click(screen.getByRole('button', { name: 'Next' }));
    await user.clear(screen.getByRole('textbox', { name: 'Manager name' }));
    await user.type(
      screen.getByRole('textbox', { name: 'Manager name' }),
      'Nova',
    );
    await user.click(screen.getByRole('radio', { name: /^Guided/ }));
    await user.type(
      screen.getByRole('textbox', { name: 'Workspace preferences' }),
      'Prioritize accessibility',
    );
    await user.click(screen.getByRole('button', { name: 'Back' }));
    await user.click(screen.getByRole('button', { name: 'Generate team' }));
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Edit Editor' })).toBeVisible(),
    );
    expect(generate).toHaveBeenCalledWith(
      expect.objectContaining({
        name: 'Acme',
        provider: 'openai',
        description: 'Build calm tools',
        maxTeamSize: 6,
      }),
    );
    expect(bootstrap).not.toHaveBeenCalled();
    expect(screen.getByRole('button', { name: 'Edit Editor' })).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByRole('textbox', { name: 'Manager name' })).toHaveValue(
      'Nova',
    );
    expect(
      screen.getByRole('textbox', { name: 'Workspace preferences' }),
    ).toHaveValue('Prioritize accessibility');
    await user.click(screen.getByRole('radio', { name: /^Observe/ }));
    await user.click(screen.getByRole('button', { name: 'Next' }));
    await user.click(screen.getByRole('button', { name: 'Create agency' }));
    await waitFor(() => expect(bootstrap).toHaveBeenCalled());
    expect(bootstrap.mock.calls[0][0].agent.name).toBe('Nova');
    expect(bootstrap.mock.calls[0][0].agent.system).toContain(
      'workspace manager',
    );
    expect(bootstrap.mock.calls[0][0].agent.system).toContain('Lead the work');
    expect(bootstrap.mock.calls[0][0].agent.system).toContain(
      'Prioritize accessibility',
    );
    expect(bootstrap.mock.calls[0][0].agent.system).toContain(
      'Initiative: guided',
    );
    expect(bootstrap.mock.calls[0][0].workers?.[0].tools).toEqual(
      toolNamesForProfile('observe'),
    );
    expect(bootstrap.mock.calls[0][0].agent.tools).toEqual(
      toolNamesForProfile('observe'),
    );
  });

  it('keeps an oversized template editable until it fits a lowered team limit', async () => {
    const user = userEvent.setup();
    renderFlow();
    await user.click(screen.getByRole('button', { name: /Marketing Agency/ }));
    await user.type(
      screen.getByRole('textbox', { name: 'Office location' }),
      '/tmp/marketing',
    );
    await user.click(screen.getByRole('button', { name: 'Next' }));
    await user.click(screen.getByRole('button', { name: 'Next' }));
    await user.selectOptions(
      screen.getByRole('combobox', { name: 'Maximum team size' }),
      '2',
    );
    await user.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByRole('alert')).toHaveTextContent(
      'exceeds the team size limit',
    );
    await user.click(screen.getByRole('button', { name: 'Remove Copywriter' }));
    await user.click(screen.getByRole('button', { name: 'Remove Analyst' }));
    await user.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByRole('textbox', { name: 'Manager name' })).toBeVisible();
  });

  it('allows returning from Team to correct an empty manager name', async () => {
    const user = userEvent.setup();
    renderFlow();
    await user.click(screen.getByRole('button', { name: /Marketing Agency/ }));
    await user.type(
      screen.getByRole('textbox', { name: 'Office location' }),
      '/tmp/marketing',
    );
    await user.click(screen.getByRole('button', { name: 'Next' }));
    await user.click(screen.getByRole('button', { name: 'Next' }));
    await user.click(screen.getByRole('button', { name: 'Next' }));
    await user.clear(screen.getByRole('textbox', { name: 'Manager name' }));
    await user.click(screen.getByRole('button', { name: 'Back' }));
    await user.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByRole('textbox', { name: 'Manager name' })).toHaveValue(
      '',
    );
    await user.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByRole('textbox', { name: 'Manager name' })).toHaveFocus();
    await user.type(
      screen.getByRole('textbox', { name: 'Manager name' }),
      'Nova',
    );
    await user.click(screen.getByRole('button', { name: 'Next' }));
    expect(
      screen.getByRole('heading', { name: 'Review', exact: true }),
    ).toBeVisible();
  });

  it('keeps the team when generation fails and allows retry', async () => {
    const user = userEvent.setup();
    vi.spyOn(daemon, 'generateAgency').mockRejectedValue(
      new Error('Provider unavailable'),
    );
    renderFlow();
    await user.click(screen.getByRole('button', { name: /Marketing Agency/ }));
    await user.type(
      screen.getByRole('textbox', { name: 'Office location' }),
      '/tmp/marketing',
    );
    await user.click(screen.getByRole('button', { name: 'Next' }));
    await user.click(screen.getByRole('button', { name: 'Next' }));
    await user.click(screen.getByRole('button', { name: 'Generate team' }));
    await waitFor(() =>
      expect(screen.getByRole('alert')).toHaveTextContent(
        'Provider unavailable',
      ),
    );
    expect(
      screen.getByRole('button', { name: 'Edit Strategist' }),
    ).toBeVisible();
    expect(screen.getByRole('button', { name: 'Generate team' })).toBeEnabled();
  });

  it('creates a template team with edited specialists and the chosen access', async () => {
    const user = userEvent.setup();
    const bootstrap = vi.spyOn(daemon, 'bootstrapWorkspace').mockResolvedValue({
      agent: snapshot(),
      workspace: { ...WORKSPACE, values: [] },
    });
    const { onCreated } = renderFlow();
    await user.click(screen.getByRole('button', { name: /Creator Studio/ }));
    expect(screen.getByRole('textbox', { name: 'Company name' })).toHaveValue(
      'My Creator Studio',
    );
    await user.type(
      screen.getByRole('textbox', { name: 'Office location' }),
      '/tmp/studio',
    );
    await user.click(screen.getByRole('button', { name: 'Next' }));
    await user.click(screen.getByRole('button', { name: 'Next' }));
    expect(
      screen.queryByRole('textbox', { name: 'Manager name' }),
    ).not.toBeInTheDocument();
    await user.click(
      screen.getByRole('button', { name: 'Edit Content Planner' }),
    );
    const planner = screen.getByRole('textbox', { name: 'Specialist 1 name' });
    await user.clear(planner);
    await user.type(planner, 'Editorial Planner');
    await user.click(
      screen.getByRole('button', { name: 'Remove Community Manager' }),
    );
    await user.click(screen.getByRole('button', { name: 'Next' }));
    await user.click(screen.getByRole('button', { name: 'Next' }));
    expect(
      within(
        screen.getByRole('region', { name: 'Review', exact: true }),
      ).getByText('Editorial Planner'),
    ).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Create agency' }));
    await waitFor(() => expect(onCreated).toHaveBeenCalled());
    const payload = bootstrap.mock.calls[0][0];
    expect(payload.workers?.map((worker) => worker.name)).toEqual([
      'Editorial Planner',
      'Scriptwriter',
    ]);
    expect(
      payload.workers?.every(
        (worker) =>
          JSON.stringify(worker.tools) ===
          JSON.stringify(toolNamesForProfile('collaborate')),
      ),
    ).toBe(true);
    expect(payload.agent.system).toContain('Content calendar');
  });

  it('requires specialist uniqueness on Team and manager uniqueness on Manager', async () => {
    const user = userEvent.setup();
    renderFlow();
    await user.click(screen.getByRole('button', { name: /Life Agency/ }));
    await user.type(
      screen.getByRole('textbox', { name: 'Office location' }),
      '/tmp/life',
    );
    await user.click(screen.getByRole('button', { name: 'Next' }));
    await user.click(screen.getByRole('button', { name: 'Next' }));
    await user.click(screen.getByRole('button', { name: 'Edit Planner' }));
    const worker = screen.getByRole('textbox', { name: 'Specialist 1 name' });
    await user.clear(worker);
    await user.type(worker, 'Research Assistant');
    await user.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByRole('alert')).toHaveTextContent('unique');
    expect(
      screen.getByRole('textbox', { name: 'Specialist 1 name' }),
    ).toBeVisible();
    await user.clear(worker);
    await user.type(worker, 'Anima');
    await user.click(screen.getByRole('button', { name: 'Next' }));
    await user.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByRole('alert')).toHaveTextContent('unique');
    expect(screen.getByRole('textbox', { name: 'Manager name' })).toBeVisible();
  });

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
    expect(screen.getAllByRole('listitem')).toHaveLength(4);
    expect(screen.getAllByRole('listitem')[0]).toHaveAttribute(
      'aria-current',
      'step',
    );
    expect(screen.getByRole('status')).toHaveTextContent(
      'Step 1 of 4: Workspace',
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
      'Enter a company name, workspace brief, and workspace folder.',
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
    expect(screen.getByRole('heading', { name: 'Model' })).toBeVisible();
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
      screen.getByRole('textbox', { name: 'Workspace brief' }),
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
      'Enter a company name, workspace brief, and workspace folder.',
    );
    expect(
      screen.getByRole('textbox', { name: 'Workspace brief' }),
    ).toHaveFocus();

    await user.type(
      screen.getByRole('textbox', { name: 'Workspace brief' }),
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

  it('preserves every draft through Workspace, Model, Manager with access, and Review', async () => {
    const user = userEvent.setup();
    renderFlow();

    expect(screen.getAllByRole('listitem')).toHaveLength(4);
    expect(screen.getByRole('status')).toHaveTextContent(
      'Step 1 of 4: Workspace',
    );

    await fillWorkspace(user);
    await user.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByRole('status')).toHaveTextContent('Step 2 of 4: Model');

    await user.selectOptions(
      screen.getByRole('combobox', { name: 'Model' }),
      'gpt-4.1',
    );
    await user.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByRole('status')).toHaveTextContent(
      'Step 3 of 4: Manager',
    );

    const name = screen.getByRole('textbox', { name: 'Manager name' });
    await user.clear(name);
    await user.type(name, 'Nova');
    await user.type(
      screen.getByRole('textbox', { name: 'Workspace preferences' }),
      'Keep Fridays free',
    );
    await user.click(screen.getByRole('radio', { name: /^Guided/ }));
    await user.click(screen.getByRole('radio', { name: /^Detailed/ }));

    await user.click(screen.getByRole('radio', { name: /Operate/ }));
    await user.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByRole('heading', { name: 'Review' })).toBeVisible();
    expect(screen.getAllByRole('listitem')[3]).toHaveAttribute(
      'aria-current',
      'step',
    );
    expect(screen.getByRole('status')).toHaveTextContent('Step 4 of 4: Launch');

    await user.click(screen.getByRole('button', { name: 'Back' }));
    expect(screen.getByRole('radio', { name: /Operate/ })).toBeChecked();
    expect(screen.getByRole('textbox', { name: 'Manager name' })).toHaveValue(
      'Nova',
    );
    expect(
      screen.getByRole('textbox', { name: 'Workspace preferences' }),
    ).toHaveValue('Keep Fridays free');
    expect(screen.getByRole('radio', { name: /^Guided/ })).toBeChecked();
    expect(screen.getByRole('radio', { name: /^Detailed/ })).toBeChecked();
    await user.click(screen.getByRole('button', { name: 'Back' }));
    expect(screen.getByRole('combobox', { name: 'Model' })).toHaveValue(
      'gpt-4.1',
    );
    await user.click(screen.getByRole('button', { name: 'Back' }));
    expect(screen.getByRole('textbox', { name: 'Company name' })).toHaveValue(
      WORKSPACE.companyName,
    );
    expect(
      screen.getByRole('textbox', { name: 'Workspace brief' }),
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

    expect(screen.getByRole('heading', { name: 'Model' })).toBeVisible();
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
      screen.getByRole('textbox', { name: 'Workspace brief' }),
    ).toHaveValue(WORKSPACE.mission);
    expect(
      screen.getByRole('textbox', { name: 'Office location' }),
    ).toHaveValue(WORKSPACE.rootPath);
  });

  it('summarizes the workspace, manager role and settings, provider/model, and access on Review', async () => {
    const user = userEvent.setup();
    renderFlow();
    await goToManager(user);

    const name = screen.getByRole('textbox', { name: 'Manager name' });
    await user.clear(name);
    await user.type(name, 'Nova');
    await user.click(screen.getByRole('radio', { name: /Operate/ }));
    await user.click(screen.getByRole('button', { name: 'Next' }));

    expect(
      within(
        screen.getByRole('region', { name: 'Review', exact: true }),
      ).getByText(WORKSPACE.companyName),
    ).toBeVisible();
    expect(
      within(
        screen.getByRole('region', { name: 'Review', exact: true }),
      ).getByText(WORKSPACE.mission),
    ).toBeVisible();
    const rootPath = screen.getByTitle(WORKSPACE.rootPath);
    expect(rootPath).toHaveTextContent(WORKSPACE.rootPath);
    expect(
      within(
        screen.getByRole('region', { name: 'Review', exact: true }),
      ).getByText('Workspace Manager'),
    ).toBeVisible();
    expect(screen.getByText(/^Balanced$/i)).toBeVisible();
    expect(screen.getByText(/^Concise$/i)).toBeVisible();

    expect(
      within(
        screen.getByRole('region', { name: 'Review', exact: true }),
      ).getByText('Nova'),
    ).toBeVisible();
    expect(
      within(
        screen.getByRole('region', { name: 'Review', exact: true }),
      ).getByText('OpenAI / gpt-4o'),
    ).toBeVisible();
    expect(
      within(
        screen.getByRole('region', { name: 'Review', exact: true }),
      ).getByText('Operate'),
    ).toBeVisible();
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
    await goToManager(user);

    const name = screen.getByRole('textbox', { name: 'Manager name' });
    await user.clear(name);
    await user.type(name, '  Nova  ');
    await user.click(screen.getByRole('radio', { name: /Operate/ }));
    await user.click(screen.getByRole('button', { name: 'Next' }));
    await user.click(screen.getByRole('button', { name: 'Create workspace' }));

    const creatingButton = await screen.findByRole('button', {
      name: 'Creating workspace…',
    });
    expect(creatingButton).toBeDisabled();
    await user.click(creatingButton);
    expect(bootstrapWorkspace).toHaveBeenCalledTimes(1);

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
        bio: expect.any(String),
        adjectives: expect.any(Array),
        style: expect.any(String),
        system: expect.stringContaining(WORKSPACE.mission),
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
      screen.getByRole('textbox', { name: 'Workspace brief' }),
    ).toHaveValue('Ship calmly');
    expect(
      screen.getByRole('textbox', { name: 'Office location' }),
    ).toHaveValue('/srv/acme');
    await waitFor(() =>
      expect(screen.getByRole('textbox', { name: /Values/ })).toHaveValue(
        'cite sources',
      ),
    );
    expect(
      screen.getByText('Your workspace is ready — set up its manager.'),
    ).toBeVisible();

    await user.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByRole('heading', { name: 'Model' })).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Next' }));
    const name = screen.getByRole('textbox', { name: 'Manager name' });
    await user.clear(name);
    await user.type(name, 'Nova');
    await user.click(screen.getByRole('radio', { name: /Operate/ }));
    await user.click(screen.getByRole('button', { name: 'Next' }));

    expect(screen.getByRole('heading', { name: 'Review' })).toBeVisible();
    expect(screen.queryByText(/anima\.yaml/)).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Create manager' }));

    await waitFor(() => expect(onCreated).toHaveBeenCalledWith(created));
    expect(createAgent).toHaveBeenCalledTimes(1);
    expect(createAgent).toHaveBeenCalledWith({
      settings: { additional: { workspaceRole: 'lead' } },
      name: 'Nova',
      provider: 'openai',
      model: 'gpt-4o',
      system: expect.stringContaining('Ship calmly'),
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
    await goToManager(user);

    const name = screen.getByRole('textbox', { name: 'Manager name' });
    await user.clear(name);
    await user.type(name, 'Nova');
    await user.click(screen.getByRole('radio', { name: /Operate/ }));
    await user.click(screen.getByRole('button', { name: 'Next' }));
    await user.click(screen.getByRole('button', { name: 'Create workspace' }));

    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('bootstrap refused');
    expect(alert).toHaveFocus();
    expect(screen.getByRole('heading', { name: 'Review' })).toBeVisible();
    expect(
      within(
        screen.getByRole('region', { name: 'Review', exact: true }),
      ).getByText('Nova'),
    ).toBeVisible();
    expect(
      within(
        screen.getByRole('region', { name: 'Review', exact: true }),
      ).getByText('OpenAI / gpt-4o'),
    ).toBeVisible();
    expect(
      within(
        screen.getByRole('region', { name: 'Review', exact: true }),
      ).getByText('Operate'),
    ).toBeVisible();
    expect(onCreated).not.toHaveBeenCalled();

    await user.click(screen.getByRole('button', { name: 'Back' }));
    expect(screen.getByRole('radio', { name: /Operate/ })).toBeChecked();
    expect(screen.getByRole('textbox', { name: 'Manager name' })).toHaveValue(
      'Nova',
    );
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

    expect(screen.getByRole('heading', { name: 'Model' })).toBeVisible();
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
    expect(
      screen.getByRole('heading', { name: 'Workspace Manager' }),
    ).toBeVisible();

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
    expect(
      within(
        screen.getByRole('region', { name: 'Review', exact: true }),
      ).getByText('OpenAI / gpt-4o'),
    ).toBeVisible();

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

    expect(await screen.findByRole('heading', { name: 'Model' })).toBeVisible();
    expect(screen.getByRole('alert')).toHaveTextContent(
      'Provider catalog changed. Review your provider and model before creating the workspace manager.',
    );
    const ollama = screen.getByRole('button', {
      name: /Ollama.*configured/i,
    });
    await waitFor(() => expect(ollama).toHaveAttribute('aria-pressed', 'true'));
    expect(ollama).toHaveFocus();
    expect(
      screen.queryByRole('button', { name: 'Create workspace' }),
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
      screen.queryByRole('textbox', { name: 'Workspace brief' }),
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
      screen.getByRole('textbox', { name: 'Workspace brief' }),
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
