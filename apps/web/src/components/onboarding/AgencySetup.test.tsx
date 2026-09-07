import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import {
  daemon,
  type DaemonProvider,
  type DaemonSnapshot,
} from '../../lib/daemon-api';
import { OnboardingFlow } from './OnboardingFlow';
import { AGENCY_TEMPLATES } from '../../lib/agency-templates';
import { DaemonHttpError } from '@animaOS-SWARM/sdk';

it('renames full names and their short references, and repairs an already mismatched draft', async () => {
  const generate = vi.spyOn(daemon, 'generateAgency').mockResolvedValue({
    name: 'Life team', agents: [
      { name: 'Director', role: 'orchestrator', bio: 'Coordinate', system: 'Ask Luis Garcia and Hana for plans.' },
      { name: 'Luis Garcia', role: 'worker', bio: 'Luis converts broad goals into small actions.', system: 'Coordinate with Hana Sato. Luis owns the plan.' },
      { name: 'Hana Sato', role: 'worker', bio: 'Hana researches open decisions.', system: "Review Luis’s plan with Hana." },
    ],
  });
  const user = await team();
  await user.click(screen.getByRole('button', { name: 'Generate team' }));
  await user.click(await screen.findByRole('button', { name: 'Edit Luis Garcia' }));
  const name = screen.getByRole('textbox', { name: 'Specialist 1 name' });
  await user.clear(name);
  await user.type(name, 'Alice');
  await user.click(screen.getByRole('button', { name: 'Edit Hana Sato' }));
  expect(screen.getByRole('textbox', { name: 'Specialist 2 instructions' })).toHaveValue('Review Alice’s plan with Hana.');
  const second = screen.getByRole('textbox', { name: 'Specialist 2 name' });
  await user.clear(second);
  await user.type(second, 'Aiko Hana');
  await user.tab();
  expect(screen.getByRole('textbox', { name: 'Specialist 2 role' })).toHaveValue('Aiko Hana researches open decisions.');
  await user.click(screen.getByRole('button', { name: 'Edit Alice' }));
  const role = screen.getByRole('textbox', { name: 'Specialist 1 role' });
  expect(role).toHaveValue('Alice converts broad goals into small actions.');
  // Simulate prose left over from a rename made before name history existed.
  await user.clear(role);
  await user.type(role, 'Luis converts broad goals into small actions.');
  await user.click(screen.getByText('Fix an old name in the text'));
  const previous = screen.getByRole('textbox', { name: 'Specialist 1 previous name' });
  await user.type(previous, 'Aiko');
  await user.click(screen.getByRole('button', { name: 'Update name references' }));
  expect(screen.getByText(/That name belongs to another team member/)).toBeVisible();
  await user.clear(previous);
  await user.type(previous, 'Unknown');
  await user.click(screen.getByRole('button', { name: 'Update name references' }));
  expect(screen.getByText(/No matching old-name references found/)).toBeVisible();
  await user.clear(previous);
  await user.type(previous, 'Luis Garcia');
  await user.click(screen.getByRole('button', { name: 'Update name references' }));
  expect(role).toHaveValue('Alice converts broad goals into small actions.');
  expect(screen.getByText('Updated name references to Alice.')).toBeVisible();
  expect(generate).toHaveBeenCalledOnce();
});

it('shows Alice and Aiko consistently on Review after renaming Mateo and Priya', async () => {
  vi.spyOn(daemon, 'generateAgency').mockResolvedValue({
    name: 'Life team', agents: [
      { name: 'Director', role: 'orchestrator', bio: 'Coordinate', system: 'Coordinate Mateo and Priya.' },
      { name: 'Mateo', role: 'worker', bio: 'Mateo translates priorities into small, sequenced actions. He spots hidden effort.', system: 'Own the conversion of agreed priorities into realistic next actions.' },
      { name: 'Priya', role: 'worker', bio: 'Priya helps compare open choices. She clarifies criteria.', system: 'Own the analysis of open decisions.' },
    ],
  });
  const user = await team();
  await user.click(screen.getByRole('button', { name: 'Generate team' }));
  await user.click(await screen.findByRole('button', { name: 'Edit Mateo' }));
  const first = screen.getByRole('textbox', { name: 'Specialist 1 name' });
  await user.clear(first);
  await user.type(first, 'Alice');
  await user.click(screen.getByRole('button', { name: 'Edit Priya' }));
  const second = screen.getByRole('textbox', { name: 'Specialist 2 name' });
  await user.clear(second);
  await user.type(second, 'Aiko');
  await user.click(screen.getByRole('button', { name: 'Next' }));
  await user.click(screen.getByRole('button', { name: 'Next' }));
  await user.click(screen.getByText('Alice', { selector: 'summary' }));
  await user.click(screen.getByText('Aiko', { selector: 'summary' }));
  expect(screen.getByText('Alice translates priorities into small, sequenced actions. He spots hidden effort.')).toBeVisible();
  expect(screen.getByText('Aiko helps compare open choices. She clarifies criteria.')).toBeVisible();
  expect(screen.queryByText(/Mateo translates|Priya helps/)).not.toBeInTheDocument();
});

it('synchronizes completed renames in roles, instructions and teammate handoffs without regenerating', async () => {
  const generate = vi.spyOn(daemon, 'generateAgency').mockResolvedValue({
    name: 'Studio', agents: [
      { name: 'Director', role: 'orchestrator', bio: 'Coordinate', system: 'You are Director. Ask Amar to draft and Bea to review.' },
      { name: 'Amar', role: 'worker', bio: "Amar owns drafts.", system: "You are Amar. Preserve amaranth research. Send Amar’s drafts to Bea." },
      { name: 'Bea', role: 'worker', bio: 'Review with Amar.', system: "Check Amar's drafts with Director." },
    ],
  });
  const bootstrap = vi.spyOn(daemon, 'bootstrapWorkspace').mockResolvedValue({
    agent: {} as DaemonSnapshot,
    workspace: { companyName: 'Studio', mission: 'Draft articles', rootPath: '/tmp/studio', values: [] },
  });
  const user = await team();
  await user.click(screen.getByRole('button', { name: 'Generate team' }));
  await user.click(await screen.findByRole('button', { name: 'Edit Amar' }));
  const name = screen.getByRole('textbox', { name: 'Specialist 1 name' });
  await user.clear(name);
  await user.tab();
  await user.type(name, 'Elise');
  await user.tab();
  expect(screen.getByRole('textbox', { name: 'Specialist 1 role' })).toHaveValue('Elise owns drafts.');
  expect(screen.getByRole('textbox', { name: 'Specialist 1 instructions' })).toHaveValue('You are Elise. Preserve amaranth research. Send Elise’s drafts to Bea.');
  await user.click(screen.getByRole('button', { name: 'Edit Bea' }));
  expect(screen.getByRole('textbox', { name: 'Specialist 2 instructions' })).toHaveValue("Check Elise's drafts with Anima.");
  await user.click(screen.getByRole('button', { name: 'Next' }));
  const manager = screen.getByRole('textbox', { name: 'Manager name' });
  await user.clear(manager);
  await user.type(manager, 'Nova');
  await user.click(screen.getByRole('button', { name: 'Next' }));
  await user.click(screen.getByRole('button', { name: 'Back' }));
  await user.click(screen.getByRole('button', { name: 'Back' }));
  await user.click(screen.getByRole('button', { name: 'Edit Bea' }));
  expect(screen.getByRole('textbox', { name: 'Specialist 2 instructions' })).toHaveValue("Check Elise's drafts with Nova.");
  expect(generate).toHaveBeenCalledOnce();
  await user.click(screen.getByRole('button', { name: 'Next' }));
  await user.click(screen.getByRole('button', { name: 'Next' }));
  await user.click(screen.getByRole('button', { name: 'Create agency' }));
  await waitFor(() => expect(bootstrap).toHaveBeenCalledOnce());
  const request = bootstrap.mock.calls[0][0];
  expect(request.agent.system).toContain('You are Nova. Ask Elise to draft and Bea to review.');
  expect(request.agent.system).not.toContain(AGENCY_TEMPLATES[0].members[0].name);
  expect(request.workers?.[0].system).toContain('You are Elise.');
  expect(request.workers?.[1].system).toContain("Check Elise's drafts with Nova.");
  expect(JSON.stringify(request)).not.toContain('referenceName');
});

it('clears old success when retrying and keeps the prior team after a generation timeout', async () => {
  let reject!: (error: Error) => void;
  const retry = new Promise<never>((_, rejectPromise) => {
    reject = rejectPromise;
  });
  vi.spyOn(daemon, 'generateAgency')
    .mockResolvedValueOnce({
      name: 'Studio',
      agents: [
        {
          name: 'Lead',
          role: 'orchestrator',
          bio: 'Coordinate',
          system: 'Coordinate',
        },
        { name: 'Writer', role: 'worker', bio: 'Write', system: 'Write' },
      ],
    })
    .mockReturnValueOnce(retry);
  const user = await team();
  await user.click(screen.getByRole('button', { name: 'Generate team' }));
  expect(
    await screen.findByText(
      'Your team preview is ready. Review it before creating.',
    ),
  ).toBeVisible();
  await user.click(screen.getByRole('button', { name: 'Generate team' }));
  expect(
    screen.queryByText(
      'Your team preview is ready. Review it before creating.',
    ),
  ).not.toBeInTheDocument();
  reject(new DaemonHttpError(408, null));
  expect(await screen.findByRole('alert')).toHaveTextContent(
    'Team generation timed out',
  );
  expect(
    screen.queryByText(
      'Your team preview is ready. Review it before creating.',
    ),
  ).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Edit Writer' })).toBeVisible();
  expect(screen.getByRole('button', { name: 'Generate team' })).toBeEnabled();
  await user.click(screen.getByRole('button', { name: 'Next' }));
  expect(screen.getByRole('heading', { name: 'Workspace' })).toBeVisible();
});

it.each(AGENCY_TEMPLATES)(
  'keeps the picker visible and fills the complete $name template',
  async (template) => {
    const user = userEvent.setup();
    render(
      <OnboardingFlow
        providers={providers}
        providersError={null}
        retryProviders={vi.fn()}
        onCreated={vi.fn()}
      />,
    );
    await user.click(
      screen.getByRole('button', { name: new RegExp(template.name) }),
    );
    for (const option of AGENCY_TEMPLATES) {
      expect(
        screen.getByRole('button', { name: new RegExp(option.name) }),
      ).toBeVisible();
    }
    expect(screen.getByRole('textbox', { name: 'Company name' })).toHaveValue(
      `My ${template.name}`,
    );
    const brief = (
      screen.getByRole('textbox', {
        name: 'Workspace brief',
      }) as HTMLTextAreaElement
    ).value;
    expect(brief).toContain(template.mission);
    expect(brief).not.toContain(template.firstTask);
    for (const deliverable of template.deliverables)
      expect(brief).toContain(deliverable);
    for (const step of template.workflow) expect(brief).toContain(step);
    expect(screen.getByRole('textbox', { name: /Values/ })).toHaveValue(
      template.values.join(', '),
    );
    await user.type(
      screen.getByRole('textbox', { name: 'Workspace brief' }),
      '\nMy own detail.',
    );
    await user.click(
      screen.getByRole('button', { name: new RegExp(template.name) }),
    );
    expect(
      (
        screen.getByRole('textbox', {
          name: 'Workspace brief',
        }) as HTMLTextAreaElement
      ).value,
    ).toContain('My own detail.');
  },
);

const providers: DaemonProvider[] = [
  {
    id: 'openai',
    label: 'OpenAI',
    configured: true,
    requiresKey: true,
    apiKeyEnvs: [],
  },
  {
    id: 'ollama',
    label: 'Ollama',
    configured: true,
    requiresKey: false,
    apiKeyEnvs: [],
  },
];

beforeEach(() => {
  vi.spyOn(daemon, 'getWorkspace').mockResolvedValue({
    configured: false,
    workspace: null,
    defaultRoot: '/tmp/studio',
  });
});
afterEach(() => vi.restoreAllMocks());

async function team() {
  const user = userEvent.setup();
  render(
    <OnboardingFlow
      providers={providers}
      providersError={null}
      retryProviders={vi.fn()}
      onCreated={vi.fn()}
    />,
  );
  await user.click(screen.getByRole('button', { name: /Marketing Agency/ }));
  await user.click(screen.getByRole('button', { name: 'Next' }));
  await user.click(screen.getByRole('button', { name: 'Next' }));
  return user;
}

it('preserves role settings and an edited prepared assignment in atomic bootstrap without running it', async () => {
  const bootstrap = vi.spyOn(daemon, 'bootstrapWorkspace').mockResolvedValue({
    agent: {} as DaemonSnapshot,
    workspace: {
      companyName: 'Studio',
      mission: 'Help',
      rootPath: '/tmp/studio',
      values: [],
    },
  });
  const user = await team();
  await user.click(screen.getByRole('button', { name: 'Edit Strategist' }));
  await user.selectOptions(
    screen.getByRole('combobox', { name: 'Specialist 1 provider' }),
    'ollama',
  );
  await user.type(
    screen.getByRole('textbox', { name: 'Specialist 1 model' }),
    'local-research',
  );
  await user.selectOptions(
    screen.getByRole('combobox', { name: 'Specialist 1 access' }),
    'observe',
  );
  await user.click(screen.getByText('Choose individual tools'));
  await user.click(screen.getByRole('checkbox', { name: 'read_file' }));
  await user.click(screen.getByRole('button', { name: 'Next' }));
  await user.click(screen.getByRole('radio', { name: /^Operate/ }));
  await user.click(screen.getByRole('button', { name: 'Next' }));
  const first = screen.getByRole('textbox', {
    name: 'First assignment (optional)',
  });
  expect(first).not.toHaveValue('');
  await user.clear(first);
  await user.type(first, 'Prepare an audience brief using supplied research.');
  await user.click(screen.getByRole('button', { name: 'Create agency' }));
  await waitFor(() => expect(bootstrap).toHaveBeenCalledOnce());
  const input = bootstrap.mock.calls[0][0];
  expect(input.workers?.[0]).toMatchObject({
    provider: 'ollama',
    model: 'local-research',
  });
  expect(input.workers?.[0].tools).not.toContain('bash');
  expect(input.workers?.[0].tools).not.toContain('read_file');
  expect(input.workers?.[0].tools).toContain('list_dir');
  expect(input.agent.tools).toContain('bash');
  expect(input.agent.system).toContain(
    'Prepare an audience brief using supplied research.',
  );
  expect(input.agent.system).toContain('do not start until the owner asks');
  expect(input.agent.system).toContain('Team responsibilities:');
  expect(input.workers?.[0]).not.toHaveProperty('suggestedTools');
});

it('requires a model when switching a specialist to another provider', async () => {
  const user = await team();
  await user.click(screen.getByRole('button', { name: 'Edit Strategist' }));
  await user.selectOptions(
    screen.getByRole('combobox', { name: 'Specialist 1 provider' }),
    'ollama',
  );
  await user.click(screen.getByRole('button', { name: 'Next' }));
  expect(screen.getByRole('alert')).toHaveTextContent(
    'Enter a model for Strategist',
  );
});

it('updates inherited models when setup changes while keeping deliberate agent overrides', async () => {
  const bootstrap = vi.spyOn(daemon, 'bootstrapWorkspace').mockResolvedValue({
    agent: {} as DaemonSnapshot,
    workspace: {
      companyName: 'Studio',
      mission: 'Help',
      rootPath: '/tmp/studio',
      values: [],
    },
  });
  const user = await team();
  await user.click(screen.getByRole('button', { name: 'Edit Strategist' }));
  await user.type(
    screen.getByRole('textbox', { name: 'Specialist 1 model' }),
    'deliberate-specialist-model',
  );
  await user.click(screen.getByRole('button', { name: 'Back' }));
  await user.selectOptions(
    screen.getByRole('combobox', { name: 'Model' }),
    'gpt-4.1',
  );
  await user.click(screen.getByRole('button', { name: 'Next' }));
  await user.click(screen.getByRole('button', { name: 'Next' }));
  await user.click(screen.getByRole('button', { name: 'Next' }));
  await user.click(screen.getByRole('button', { name: 'Create agency' }));
  await waitFor(() => expect(bootstrap).toHaveBeenCalledOnce());
  const input = bootstrap.mock.calls[0][0];
  expect(input.agent).toMatchObject({ provider: 'openai', model: 'gpt-4.1' });
  expect(input.workers?.[0]).toMatchObject({
    provider: 'openai',
    model: 'deliberate-specialist-model',
  });
  expect(
    input.workers
      ?.slice(1)
      .every(
        (worker) => worker.model === 'gpt-4.1' && worker.provider === 'openai',
      ),
  ).toBe(true);
});

it('lets users add a specialist and requires complete responsibilities', async () => {
  const user = await team();
  await user.selectOptions(
    screen.getByRole('combobox', { name: 'Maximum team size' }),
    '5',
  );
  await user.click(screen.getByRole('button', { name: 'Add specialist' }));
  await user.click(screen.getByRole('button', { name: 'Next' }));
  expect(screen.getByRole('alert')).toHaveTextContent(
    'Every team member needs a name',
  );
  await user.click(screen.getByRole('button', { name: 'Edit specialist 4' }));
  await user.type(
    screen.getByRole('textbox', { name: 'Specialist 4 name' }),
    'Reviewer',
  );
  await user.type(
    screen.getByRole('textbox', { name: 'Specialist 4 role' }),
    'Check evidence before handoff',
  );
  await user.type(
    screen.getByRole('textbox', { name: 'Specialist 4 instructions' }),
    'Review claims and return corrections to the manager.',
  );
  await user.click(screen.getByRole('button', { name: 'Next' }));
  expect(screen.getByRole('heading', { name: 'Workspace' })).toBeVisible();
});

it('requires at least one specialist for an agency', async () => {
  const user = await team();
  for (const name of ['Strategist', 'Copywriter', 'Analyst']) {
    await user.click(screen.getByRole('button', { name: `Remove ${name}` }));
  }
  await user.click(screen.getByRole('button', { name: 'Next' }));
  expect(screen.getByRole('alert')).toHaveTextContent(
    'at least one specialist',
  );
});

it('uses the selected setup model for generated agents and ignores generated model/tool suggestions', async () => {
  vi.spyOn(daemon, 'generateAgency').mockResolvedValue({
    name: 'Studio',
    provider: 'openai',
    agents: [
      {
        name: 'Director',
        role: 'orchestrator',
        bio: 'Coordinate',
        system: 'Coordinate the work',
      },
      {
        name: 'Writer',
        role: 'worker',
        bio: 'Write drafts',
        system: 'Write carefully',
        model: 'unrequested-model',
        tools: ['bash'],
      },
    ],
  });
  const user = userEvent.setup();
  const bootstrap = vi.spyOn(daemon, 'bootstrapWorkspace').mockResolvedValue({
    agent: {} as DaemonSnapshot,
    workspace: {
      companyName: 'Studio',
      mission: 'Write useful articles',
      rootPath: '/tmp/studio',
      values: [],
    },
  });
  render(
    <OnboardingFlow
      providers={providers}
      providersError={null}
      retryProviders={vi.fn()}
      onCreated={vi.fn()}
    />,
  );
  await user.click(
    screen.getByRole('button', { name: /Create a custom agency/ }),
  );
  await user.type(
    screen.getByRole('textbox', { name: 'Company name' }),
    'Studio',
  );
  await user.type(
    screen.getByRole('textbox', { name: 'Workspace brief' }),
    'Write useful articles',
  );
  await user.click(screen.getByRole('button', { name: 'Next' }));
  await user.selectOptions(
    screen.getByRole('combobox', { name: 'Model' }),
    'gpt-4.1',
  );
  await user.click(screen.getByRole('button', { name: 'Next' }));
  await user.click(screen.getByRole('button', { name: 'Generate team' }));
  await screen.findByRole('button', { name: 'Edit Writer' });
  await user.click(screen.getByRole('button', { name: 'Next' }));
  await user.click(screen.getByRole('button', { name: 'Next' }));
  expect(
    screen.getByRole('textbox', { name: 'First assignment (optional)' }),
  ).not.toHaveValue('');
  await user.click(screen.getByRole('button', { name: 'Create agency' }));
  await waitFor(() => expect(bootstrap).toHaveBeenCalledOnce());
  expect(bootstrap.mock.calls[0][0].workers?.[0]).toMatchObject({
    provider: 'openai',
    model: 'gpt-4.1',
  });
  expect(bootstrap.mock.calls[0][0].workers?.[0].tools).not.toContain('bash');
});
