import { expect, test, type Page, type Route } from '@playwright/test';

interface FixtureMessage {
  id: string;
  agentId: string;
  roomId: string;
  role: string;
  content: { text: string; metadata?: Record<string, unknown> | null };
  createdAtMs: number;
}

interface BrowserElement {
  ownerDocument: {
    defaultView: {
      getComputedStyle(target: unknown): {
        animationDuration: string;
        animationIterationCount: string;
        transitionDuration: string;
      };
    } | null;
  };
}

const providers = [
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
    configured: true,
    apiKeyEnvs: ['ANTHROPIC_API_KEY'],
  },
];

function agentSnapshot(
  id: string,
  name: string,
  createdAtMs: number,
  tools: string[] = [],
  messages: FixtureMessage[] = [],
) {
  return {
    state: {
      id,
      name,
      status: 'idle',
      config: {
        name,
        provider: 'openai',
        model: 'gpt-4.1',
        system: 'Be precise',
        tools: tools.map((tool) => ({
          name: tool,
          description: tool,
          parameters: {},
        })),
      },
      createdAtMs,
      tokenUsage: {
        promptTokens: 0,
        completionTokens: 0,
        totalTokens: 0,
      },
    },
    messageCount: messages.length,
    messages,
    eventCount: 0,
  };
}

type AgentSnapshot = ReturnType<typeof agentSnapshot>;

async function fulfillJson(route: Route, body: unknown, status = 200) {
  await route.fulfill({
    status,
    contentType: 'application/json',
    body: JSON.stringify(body),
  });
}

async function installApiFixture(
  page: Page,
  options: {
    agents?: AgentSnapshot[];
    offline?: boolean;
    failFirstCreate?: boolean;
    failFirstProviders?: boolean;
    failFirstPatch?: boolean;
  } = {},
) {
  const state = {
    agents: [...(options.agents ?? [])],
    createAttempts: 0,
    patchAttempts: 0,
    providerAttempts: 0,
  };

  await page.route('**/api/**', async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname.replace(/^\/api/, '');

    if (options.offline && (path === '/health' || path === '/agents')) {
      await fulfillJson(route, { error: 'daemon unavailable' }, 503);
      return;
    }
    if (path === '/health') {
      await fulfillJson(route, { status: 'ok' });
      return;
    }
    if (path === '/providers') {
      state.providerAttempts += 1;
      if (options.failFirstProviders && state.providerAttempts === 1) {
        await fulfillJson(
          route,
          { error: 'provider catalog unavailable' },
          503,
        );
        return;
      }
      await fulfillJson(route, { providers });
      return;
    }
    if (path === '/agents' && request.method() === 'GET') {
      await fulfillJson(route, { agents: state.agents });
      return;
    }
    if (path === '/agents' && request.method() === 'POST') {
      state.createAttempts += 1;
      if (options.failFirstCreate && state.createAttempts === 1) {
        await fulfillJson(route, { error: 'creation refused' }, 500);
        return;
      }
      const input = request.postDataJSON() as {
        name: string;
        model: string;
        provider: string;
        system?: string;
        tools: string[];
      };
      const created = agentSnapshot(
        `agent-${state.createAttempts}`,
        input.name,
        100 + state.createAttempts,
        input.tools,
      );
      created.state.config.model = input.model;
      created.state.config.provider = input.provider;
      created.state.config.system = input.system ?? '';
      state.agents.push(created);
      await fulfillJson(route, { agent: created });
      return;
    }
    const agentMatch = path.match(/^\/agents\/([^/]+)$/);
    if (agentMatch && request.method() === 'PATCH') {
      state.patchAttempts += 1;
      if (options.failFirstPatch && state.patchAttempts === 1) {
        await fulfillJson(route, { error: 'settings refused' }, 500);
        return;
      }
      const agentIndex = state.agents.findIndex(
        (agent) => agent.state.id === agentMatch[1],
      );
      if (agentIndex === -1) {
        await fulfillJson(route, { error: 'agent not found' }, 404);
        return;
      }
      const input = request.postDataJSON() as Partial<{
        name: string;
        model: string;
        provider: string;
        system: string;
      }>;
      const updated = structuredClone(state.agents[agentIndex]);
      if (input.name !== undefined) {
        updated.state.name = input.name;
        updated.state.config.name = input.name;
      }
      if (input.model !== undefined) {
        updated.state.config.model = input.model;
      }
      if (input.provider !== undefined) {
        updated.state.config.provider = input.provider;
      }
      if (input.system !== undefined) {
        updated.state.config.system = input.system;
      }
      state.agents[agentIndex] = updated;
      await fulfillJson(route, { agent: updated });
      return;
    }

    await fulfillJson(
      route,
      { error: `unhandled fixture route: ${request.method()} ${path}` },
      404,
    );
  });

  return state;
}

async function completeDraftToReview(page: Page, name = 'Nova') {
  await page.getByRole('textbox', { name: 'Agent name' }).fill(name);
  await page.getByRole('button', { name: 'Next' }).click();
  await page.getByRole('button', { name: 'Next' }).click();
  await page.getByRole('button', { name: 'Next' }).click();
  await expect(page.getByRole('heading', { name: 'Review' })).toBeVisible();
}

function longestCssDurationSeconds(value: string) {
  return Math.max(
    ...value.split(',').map((duration) => {
      const trimmed = duration.trim();
      return trimmed.endsWith('ms')
        ? Number.parseFloat(trimmed) / 1_000
        : Number.parseFloat(trimmed);
    }),
  );
}

test('main workspace agent: zero-agent daemon shows onboarding without workspace navigation', async ({
  page,
}) => {
  await installApiFixture(page);
  await page.goto('/');

  await expect(
    page.getByRole('heading', { name: 'Create your main agent' }),
  ).toBeVisible();
  await expect(page.getByRole('navigation')).toHaveCount(0);
});

test('main workspace agent: provider retry preserves Identity and continues onboarding', async ({
  page,
}) => {
  await installApiFixture(page, { failFirstProviders: true });
  await page.goto('/');

  const name = page.getByRole('textbox', { name: 'Agent name' });
  const instructions = page.getByRole('textbox', {
    name: 'Instructions (optional)',
  });
  await name.fill('Retry Nova');
  await instructions.fill('Keep this draft');
  await page.getByRole('button', { name: 'Next' }).click();

  await expect(page.getByRole('alert')).toContainText(
    'provider catalog unavailable',
  );
  await page.getByRole('button', { name: 'Retry providers' }).click();
  await expect(page.getByRole('button', { name: /^OpenAI/ })).toBeVisible();

  await page.getByRole('button', { name: 'Back' }).click();
  await expect(name).toHaveValue('Retry Nova');
  await expect(instructions).toHaveValue('Keep this draft');
  await page.getByRole('button', { name: 'Next' }).click();
  await page.getByRole('button', { name: 'Next' }).click();
  await expect(page.getByRole('status')).toContainText('Step 3 of 4: Access');
});

test('main workspace agent: failed POST preserves the complete onboarding draft', async ({
  page,
}) => {
  await installApiFixture(page, { failFirstCreate: true });
  await page.goto('/');
  await page.getByRole('textbox', { name: 'Agent name' }).fill('Nova');
  await page
    .getByRole('textbox', { name: 'Instructions (optional)' })
    .fill('Be exact');
  await page.getByRole('button', { name: 'Next' }).click();

  await page.getByRole('button', { name: /^Anthropic/ }).click();
  await page.getByLabel('Model').selectOption('__custom__');
  await page.getByLabel('Custom model').fill('claude-review-custom');
  await page.getByRole('button', { name: 'Next' }).click();
  await page.getByRole('radio', { name: /^Operate/ }).check();
  await page.getByRole('button', { name: 'Next' }).click();

  await expect(page.getByRole('status')).toContainText('Step 4 of 4: Review');
  await expect(
    page.getByText('Anthropic / claude-review-custom'),
  ).toBeVisible();
  await expect(page.getByText('Operate', { exact: true })).toBeVisible();

  await page.getByRole('button', { name: 'Create agent' }).click();

  await expect(page.getByRole('alert')).toContainText('creation refused');
  await expect(page.getByRole('heading', { name: 'Review' })).toBeVisible();
  await expect(page.getByText('Nova')).toBeVisible();
  await expect(page.getByText('Be exact')).toBeVisible();
  await expect(
    page.getByText('Anthropic / claude-review-custom'),
  ).toBeVisible();
  await expect(page.getByText('Operate', { exact: true })).toBeVisible();
});

test('main workspace agent: successful POST transitions directly into the centered workspace', async ({
  page,
}) => {
  await installApiFixture(page);
  await page.goto('/');
  await completeDraftToReview(page, 'Nova');

  await page.getByRole('button', { name: 'Create agent' }).click();

  await expect(
    page.getByRole('heading', { name: 'Say something to Nova' }),
  ).toBeVisible();
  await expect(
    page.getByRole('navigation', { name: 'Workspace navigation' }),
  ).toBeVisible();
  await expect(page.getByText('Daemon Online')).toBeVisible();
  await expect(page.getByText('Agent Idle')).toBeVisible();
  await expect(page.getByText('Access Collaborate')).toBeVisible();
});

test('main workspace agent: failed PATCH keeps settings draft and prior main-agent state', async ({
  page,
}) => {
  const main = agentSnapshot(
    'agent-main',
    'Nova',
    1,
    [],
    [
      {
        id: 'message-1',
        agentId: 'agent-main',
        roomId: 'room-1',
        role: 'assistant',
        content: { text: 'Existing conversation' },
        createdAtMs: 2,
      },
    ],
  );
  const fixture = await installApiFixture(page, {
    agents: [main],
    failFirstPatch: true,
  });
  await page.goto('/');

  await expect(page.getByText('Existing conversation')).toBeVisible();
  await page.getByRole('button', { name: 'Settings' }).click();
  const settings = page.locator('aside');
  const name = settings.locator('input.field').first();
  const provider = settings.getByRole('combobox').nth(0);
  const model = settings.getByRole('combobox').nth(1);
  await name.fill('Nova Draft');
  await provider.selectOption('anthropic');
  await model.selectOption('__custom__');
  const customModel = settings.getByPlaceholder('model id, e.g. llama3.1');
  await customModel.fill('claude-settings-custom');
  const system = settings.getByPlaceholder(
    'Leave empty for the daemon default.',
  );
  await system.fill('Draft system prompt');

  await settings.getByRole('button', { name: 'Save changes' }).click();

  await expect(settings.getByText('settings refused')).toBeVisible();
  await expect(
    settings.getByRole('heading', { name: 'Agent settings' }),
  ).toBeVisible();
  await expect(name).toHaveValue('Nova Draft');
  await expect(provider).toHaveValue('anthropic');
  await expect(model).toHaveValue('__custom__');
  await expect(customModel).toHaveValue('claude-settings-custom');
  await expect(system).toHaveValue('Draft system prompt');
  await expect(
    page.getByRole('heading', { name: 'Nova', exact: true }),
  ).toBeVisible();
  await expect(page.getByText('Existing conversation')).toBeVisible();
  expect(fixture.agents[0]?.state.name).toBe('Nova');
  expect(fixture.agents[0]?.state.config).toMatchObject({
    name: 'Nova',
    provider: 'openai',
    model: 'gpt-4.1',
    system: 'Be precise',
  });
  expect(fixture.agents[0]?.messages).toHaveLength(1);
});

test('main workspace agent: multiple agents select the oldest then id and identify Main', async ({
  page,
}) => {
  await installApiFixture(page, {
    agents: [
      agentSnapshot('agent-later', 'Later', 20),
      agentSnapshot('agent-b', 'Beta', 10),
      agentSnapshot('agent-a', 'Alpha', 10),
    ],
  });
  await page.goto('/');

  await expect(
    page.getByRole('heading', { name: 'Say something to Alpha' }),
  ).toBeVisible();
  const desktopNavigation = page.getByRole('navigation', {
    name: 'Workspace navigation',
  });
  expect((await desktopNavigation.boundingBox())?.y).toBeLessThan(180);
  await page.getByRole('button', { name: 'Agents' }).click();
  await expect(
    page.getByRole('article', { name: 'Alpha agent' }),
  ).toContainText('Main');
  await expect(page.getByRole('article', { name: 'Beta agent' })).toContainText(
    'Read only',
  );
});

test('main workspace agent: offline daemon shows focused recovery without onboarding', async ({
  page,
}) => {
  await installApiFixture(page, { offline: true });
  await page.goto('/');

  await expect(page.getByRole('alert')).toContainText('Offline');
  await expect(page.getByRole('alert')).toContainText('bun dev --host rust');
  await expect(
    page.getByRole('button', { name: 'Retry connection' }),
  ).toBeFocused();
  await expect(
    page.getByRole('heading', { name: 'Create your main agent' }),
  ).toHaveCount(0);
  await expect(page.getByRole('navigation')).toHaveCount(0);
});

test('main workspace agent: 390x844 viewport places the same destinations in a bottom dock', async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await installApiFixture(page, {
    agents: [agentSnapshot('agent-main', 'Nova', 1)],
  });
  await page.goto('/');

  const navigation = page.getByRole('navigation', {
    name: 'Workspace navigation',
  });
  await expect(
    navigation.getByRole('button', { name: 'Workspace' }),
  ).toBeVisible();
  await expect(
    navigation.getByRole('button', { name: 'Activity' }),
  ).toBeVisible();
  await expect(
    navigation.getByRole('button', { name: 'Agents' }),
  ).toBeVisible();
  const box = await navigation.boundingBox();
  expect(box?.y).toBeGreaterThan(740);
});

test('main workspace agent: reduced motion keeps the workspace operable with near-instant motion', async ({
  page,
}) => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await installApiFixture(page, {
    agents: [agentSnapshot('agent-main', 'Nova', 1)],
  });
  await page.goto('/');

  const orb = page.locator('[data-motion="agent-orb"]');
  await expect(orb).toBeVisible();
  const orbMotion = await orb.evaluate((element) => {
    const browserElement = element as unknown as BrowserElement;
    const style =
      browserElement.ownerDocument.defaultView?.getComputedStyle(
        browserElement,
      );
    if (!style) {
      throw new Error('orb has no browser view');
    }
    return {
      animationDuration: style.animationDuration,
      animationIterationCount: style.animationIterationCount,
    };
  });
  expect(
    longestCssDurationSeconds(orbMotion.animationDuration),
  ).toBeLessThanOrEqual(0.001);
  expect(orbMotion.animationIterationCount).toBe('1');

  const agentsDestination = page.getByRole('button', { name: 'Agents' });
  const destinationTransition = await agentsDestination.evaluate((element) => {
    const browserElement = element as unknown as BrowserElement;
    const style =
      browserElement.ownerDocument.defaultView?.getComputedStyle(
        browserElement,
      );
    if (!style) {
      throw new Error('destination has no browser view');
    }
    return style.transitionDuration;
  });
  expect(longestCssDurationSeconds(destinationTransition)).toBeLessThanOrEqual(
    0.001,
  );
  await agentsDestination.click();
  await expect(page.getByRole('heading', { name: 'Agents' })).toBeVisible();
});

test('main workspace agent: keyboard steps announce progress, focus invalid fields, and expose status labels', async ({
  page,
}) => {
  await installApiFixture(page);
  await page.goto('/');

  const name = page.getByRole('textbox', { name: 'Agent name' });
  await name.fill('');
  await page.getByRole('button', { name: 'Next' }).click();
  await expect(name).toBeFocused();
  await expect(page.getByRole('alert')).toContainText('Enter an agent name.');

  await name.fill('Nova');
  await page.getByRole('button', { name: 'Next' }).focus();
  await page.keyboard.press('Enter');
  await expect(page.getByRole('status')).toContainText(
    'Step 2 of 4: Intelligence',
  );
  await page.getByRole('button', { name: 'Next' }).click();
  await expect(page.getByRole('status')).toContainText('Step 3 of 4: Access');
  await page.getByRole('button', { name: 'Next' }).click();
  await expect(page.getByRole('status')).toContainText('Step 4 of 4: Review');
  await page.getByRole('button', { name: 'Create agent' }).click();

  await expect(page.getByLabel('Daemon online')).toBeVisible();
  await expect(page.getByLabel('Agent idle')).toBeVisible();
  await expect(page.getByLabel('Collaborate access profile')).toBeVisible();
});
