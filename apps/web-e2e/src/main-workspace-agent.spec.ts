import { expect, test, type Page, type Route } from '@playwright/test';

type AgentSnapshot = ReturnType<typeof agentSnapshot>;

const providers = [
  {
    id: 'openai',
    label: 'OpenAI',
    requiresKey: true,
    configured: true,
    apiKeyEnvs: ['OPENAI_API_KEY'],
  },
];

function agentSnapshot(
  id: string,
  name: string,
  createdAtMs: number,
  tools: string[] = [],
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
    messageCount: 0,
    messages: [],
    eventCount: 0,
  };
}

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
  } = {},
) {
  const state = {
    agents: [...(options.agents ?? [])],
    createAttempts: 0,
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

test('main workspace agent: failed POST preserves the complete onboarding draft', async ({
  page,
}) => {
  await installApiFixture(page, { failFirstCreate: true });
  await page.goto('/');
  await page
    .getByRole('textbox', { name: 'Instructions (optional)' })
    .fill('Be exact');
  await completeDraftToReview(page, 'Nova');

  await page.getByRole('button', { name: 'Create agent' }).click();

  await expect(page.getByRole('alert')).toContainText('creation refused');
  await expect(page.getByRole('heading', { name: 'Review' })).toBeVisible();
  await expect(page.getByText('Nova')).toBeVisible();
  await expect(page.getByText('Be exact')).toBeVisible();
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
  await page.getByRole('button', { name: 'Create agent' }).click();

  await expect(page.getByLabel('Daemon online')).toBeVisible();
  await expect(page.getByLabel('Agent idle')).toBeVisible();
  await expect(page.getByLabel('Collaborate access profile')).toBeVisible();
});
