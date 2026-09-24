import { expect, test, type Page, type Route } from '@playwright/test';

interface FixtureMessage {
  id: string;
  agentId: string;
  roomId: string;
  role: string;
  content: { text: string; metadata?: Record<string, unknown> | null };
  createdAtMs: number;
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
    failAgentListsAfterFirst?: boolean;
  } = {},
) {
  const state = {
    agents: [...(options.agents ?? [])],
    createAttempts: 0,
    patchAttempts: 0,
    providerAttempts: 0,
    agentListAttempts: 0,
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
      state.agentListAttempts += 1;
      if (options.failAgentListsAfterFirst && state.agentListAttempts > 1) {
        await fulfillJson(route, { error: 'daemon unavailable' }, 503);
        return;
      }
      await fulfillJson(route, { agents: state.agents });
      return;
    }
    if (path === '/workspace') {
      await fulfillJson(route, {
        configured: false,
        workspace: null,
        defaultRoot: '/workspace',
      });
      return;
    }
    const sessionsMatch = path.match(/^\/agents\/([^/]+)\/sessions(?:\/([^/]+)(\/messages)?)?$/);
    if (sessionsMatch) {
      const owner = state.agents.find((agent) => agent.state.id === sessionsMatch[1]);
      const rooms = [...new Set((owner?.messages ?? []).map((message) => message.roomId))];
      const session = (roomId: string) => ({
        id: roomId, agentId: sessionsMatch[1], roomId, kind: 'chat', origin: 'web', title: 'Earlier chat',
        titleSource: 'first_message', createdAtMs: 1, lastActivityAtMs: 2, lastReadAtMs: 2, archived: false,
        parentSessionId: null, parentRunId: null, parentAgentId: null, summary: null, contextTrimmed: null,
        messageCount: 1, preview: null, activeRuns: 0, pendingApprovals: 0, unread: false,
        capabilities: { send: true, steer: true, stop: true, rename: true, archive: true, delete: true, compact: true, export: true },
      });
      const sessionId = sessionsMatch[2] ? decodeURIComponent(sessionsMatch[2]) : null;
      if (sessionId && sessionsMatch[3]) {
        await fulfillJson(route, {
          messages: (owner?.messages ?? [])
            .filter((message) => message.roomId === sessionId)
            .map((message) => ({ id: message.id, role: message.role, text: message.content.text, attachments: [], metadata: message.content.metadata ?? {}, createdAtMs: message.createdAtMs })),
          nextBefore: null,
        });
      } else if (sessionId) {
        await fulfillJson(route, { session: session(sessionId) });
      } else {
        await fulfillJson(route, { sessions: rooms.map(session), nextCursor: null });
      }
      return;
    }
    if (path.includes('/connectors')) {
      await fulfillJson(route, { connectors: [] });
      return;
    }
    if (path === '/workspace/bootstrap' && request.method() === 'POST') {
      state.createAttempts += 1;
      if (options.failFirstCreate && state.createAttempts === 1) {
        await fulfillJson(route, { error: 'creation refused' }, 500);
        return;
      }
      const input = request.postDataJSON().agent as {
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

test('empty workspace opens companion setup without agency navigation', async ({
  page,
}) => {
  await installApiFixture(page);
  await page.goto('/');
  await expect(
    page.getByRole('heading', { name: 'Set up your companion' }),
  ).toBeVisible();
  await expect(page.getByRole('navigation')).toHaveCount(0);
  await expect(
    page.getByRole('button', { name: 'Start chatting' }),
  ).toBeEnabled();
});

test('provider retry preserves companion identity and preferences', async ({
  page,
}) => {
  await installApiFixture(page, { failFirstProviders: true });
  await page.goto('/');
  await page.getByLabel('Companion name').fill('Retry Nova');
  await page
    .getByText('Personalize and set permissions', { exact: true })
    .click();
  const preferences = page.getByLabel(
    'What should your companion know about you?',
  );
  await preferences.fill('Keep this draft');
  await expect(page.getByRole('alert')).toContainText(
    'provider catalog unavailable',
  );
  await page.getByRole('button', { name: 'Retry providers' }).click();
  await expect(
    page.getByRole('button', { name: 'Start chatting' }),
  ).toBeEnabled();
  await expect(page.getByLabel('Companion name')).toHaveValue('Retry Nova');
  await expect(preferences).toHaveValue('Keep this draft');
});

test('failed bootstrap preserves the draft and retry creates one companion', async ({
  page,
}) => {
  const fixture = await installApiFixture(page, { failFirstCreate: true });
  await page.goto('/');
  await page.getByLabel('Companion name').fill('Nova');
  await page
    .getByText('Personalize and set permissions', { exact: true })
    .click();
  await page
    .getByLabel('What should your companion know about you?')
    .fill('Be exact');
  await page.getByRole('button', { name: /^Anthropic/ }).click();
  await page
    .getByRole('combobox', { name: 'Model', exact: true })
    .selectOption('__custom__');
  await page.getByLabel('Custom model').fill('claude-review-custom');
  await page.getByRole('radio', { name: /^Operate/ }).check();
  await page.getByRole('button', { name: 'Start chatting' }).click();
  await expect(page.getByRole('alert')).toContainText('creation refused');
  await expect(page.getByLabel('Companion name')).toHaveValue('Nova');
  await expect(
    page.getByLabel('What should your companion know about you?'),
  ).toHaveValue('Be exact');
  await expect(page.getByLabel('Custom model')).toHaveValue(
    'claude-review-custom',
  );
  await expect(page.getByRole('radio', { name: /^Operate/ })).toBeChecked();
  await page.getByRole('button', { name: 'Start chatting' }).click();
  await expect(page.getByPlaceholder('Message Nova…')).toBeVisible();
  expect(fixture.agents).toHaveLength(1);
  expect(fixture.agents[0].state.config).toMatchObject({
    provider: 'anthropic',
    model: 'claude-review-custom',
  });
  expect(fixture.agents[0].state.config.system).toContain('Be exact');
});

test('failed settings save preserves draft, conversation and original identity', async ({
  page,
}) => {
  const main = agentSnapshot(
    'main',
    'Nova',
    1,
    [],
    [
      {
        id: 'message-1',
        agentId: 'main',
        roomId: 'direct:main',
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
  await page.goto('/#/s/direct%3Amain');
  await expect(page.getByText('Existing conversation')).toBeVisible();
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  const settings = page.getByRole('dialog', { name: 'Agent settings' });
  await expect(settings).toHaveAttribute('aria-modal', 'true');
  await expect(page.getByTestId('workspace-background')).toHaveAttribute(
    'inert',
    '',
  );
  const name = settings.getByLabel('Name', { exact: true });
  await name.fill('Nova Draft');
  await settings.getByRole('button', { name: 'Save changes' }).click();
  await expect(settings.getByText('settings refused')).toBeVisible();
  await expect(name).toHaveValue('Nova Draft');
  await settings.getByRole('button', { name: 'Close settings' }).click();
  await expect(
    page.getByRole('button', { name: 'Settings', exact: true }),
  ).toBeFocused();
  await expect(page.getByText('Existing conversation')).toBeVisible();
  expect(fixture.agents[0].state.name).toBe('Nova');
  expect(fixture.agents[0].messages).toHaveLength(1);
});

test('existing agents remain intact while the oldest identity is selected', async ({
  page,
}) => {
  const fixture = await installApiFixture(page, {
    agents: [
      agentSnapshot('later', 'Later', 20),
      agentSnapshot('b', 'Beta', 10),
      agentSnapshot('a', 'Alpha', 10),
    ],
  });
  await page.goto('/');
  await expect(page.getByPlaceholder('Message Alpha…')).toBeVisible();
  await expect(
    page.getByRole('combobox', { name: 'Chat with agent' }),
  ).toHaveCount(0);
  await expect(
    page.getByRole('button', { name: 'Team', exact: true }),
  ).toHaveCount(0);
  expect(fixture.agents.map((agent) => agent.state.id)).toEqual([
    'later',
    'b',
    'a',
  ]);
  expect(fixture.createAttempts).toBe(0);
});

test('offline daemon shows recovery without onboarding', async ({ page }) => {
  await installApiFixture(page, { offline: true });
  await page.goto('/');
  await expect(page.getByRole('alert')).toContainText('Offline');
  await expect(
    page.getByRole('button', { name: 'Retry connection' }),
  ).toBeFocused();
  await expect(
    page.getByRole('heading', { name: 'Set up your companion' }),
  ).toHaveCount(0);
});

test('mobile chat keeps a bounded navigation dock and reports disconnection', async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await installApiFixture(page, {
    agents: [agentSnapshot('main', 'Nova', 1)],
    failAgentListsAfterFirst: true,
  });
  await page.goto('/');
  const navigation = page.getByRole('navigation', {
    name: 'Workspace navigation',
  });
  await expect(navigation).toHaveAttribute('data-placement', 'bottom-dock');
  await expect(
    navigation.getByRole('button', { name: 'Chats', exact: true }),
  ).toBeVisible();
  const box = await navigation.boundingBox();
  expect(box!.y).toBeGreaterThan(700);
  expect(box!.y + box!.height).toBeLessThanOrEqual(844);
  await expect(page.getByText('Offline', { exact: true })).toBeVisible({
    timeout: 7000,
  });
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
});

test('keyboard can submit companion setup after entering a valid name', async ({
  page,
}) => {
  await installApiFixture(page);
  await page.goto('/');
  const name = page.getByLabel('Companion name');
  await name.fill('');
  await expect(
    page.getByRole('button', { name: 'Start chatting' }),
  ).toBeDisabled();
  await name.fill('Keyboard Nova');
  await name.press('Enter');
  await expect(page.getByPlaceholder('Message Keyboard Nova…')).toBeVisible();
});
