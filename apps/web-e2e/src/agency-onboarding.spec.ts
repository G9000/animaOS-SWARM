import { test, expect } from '@playwright/test';

// Agency templates are no longer the front door. A configured workspace with
// only retained helpers must gain one companion without being bootstrapped again.
for (const viewport of [
  { width: 1280, height: 900 },
  { width: 390, height: 844 },
]) {
  test(`configured workspace gains one companion and retains helper data at ${viewport.width}px`, async ({
    page,
  }) => {
    await page.setViewportSize(viewport);
    const helper = {
      state: {
        id: 'retained-helper',
        name: 'Existing helper',
        status: 'idle',
        createdAtMs: 1,
        config: {
          name: 'Existing helper',
          provider: 'openai',
          model: 'test-model',
          tools: [],
          settings: {
            additional: {
              workspaceRole: 'helper',
              parentAgentId: 'former-companion',
            },
          },
        },
        tokenUsage: { promptTokens: 0, completionTokens: 0, totalTokens: 0 },
      },
      messages: [
        {
          id: 'old-result',
          role: 'assistant',
          agentId: 'retained-helper',
          roomId: 'saved-room',
          content: { text: 'Preserved helper result' },
          createdAtMs: 2,
        },
      ],
      messageCount: 1,
      eventCount: 0,
    };
    const original = structuredClone(helper);
    const agents: unknown[] = [helper];
    const mutations: string[] = [];
    let submitted: Record<string, unknown> | undefined;
    await page.route('**/api/**', async (route) => {
      const request = route.request();
      const path = new URL(request.url()).pathname;
      if (request.method() !== 'GET')
        mutations.push(`${request.method()} ${path}`);
      let body: unknown;
      if (path === '/api/health') body = { status: 'ok' };
      else if (path === '/api/workspace')
        body = {
          configured: true,
          workspace: {
            rootPath: '/saved-workspace',
            companyName: 'Existing',
            mission: 'Preserve my original mission',
            values: [],
            hasAvatar: false,
          },
          defaultRoot: '/workspace',
        };
      else if (path === '/api/providers')
        body = {
          providers: [
            {
              id: 'openai',
              label: 'OpenAI',
              configured: true,
              requiresKey: true,
              apiKeyEnvs: ['OPENAI_API_KEY'],
            },
          ],
        };
      else if (path === '/api/agents' && request.method() === 'POST') {
        submitted = request.postDataJSON();
        const created = {
          state: {
            ...helper.state,
            id: 'companion',
            name: submitted!.name,
            config: submitted,
          },
          messages: [],
          messageCount: 0,
          eventCount: 0,
        };
        agents.push(created);
        body = { agent: created };
      } else if (path === '/api/agents') body = { agents };
      else if (path.includes('/connectors')) body = { connectors: [] };
      else
        return route.fulfill({
          status: 404,
          contentType: 'application/json',
          body: JSON.stringify({ error: `Unexpected request: ${path}` }),
        });
      await route.fulfill({
        contentType: 'application/json',
        body: JSON.stringify(body),
      });
    });
    await page.goto('/');
    await expect(
      page.getByRole('heading', { name: 'Set up your companion' }),
    ).toBeVisible();
    await expect(
      page.getByRole('button', { name: /Creator Studio|Create agency/ }),
    ).toHaveCount(0);
    await page
      .getByText('Personalize and set permissions', { exact: true })
      .click();
    await expect(page.getByLabel('Workspace folder on the server')).toHaveValue(
      '/saved-workspace',
    );
    await expect(
      page.getByLabel('Workspace folder on the server'),
    ).toHaveAttribute('readonly', '');
    await page.getByLabel('Companion name').fill('Nova');
    await page.getByRole('button', { name: 'Start chatting' }).click();
    await expect(page.getByPlaceholder('Message Nova…')).toBeVisible();
    expect(mutations).toEqual(['POST /api/agents']);
    expect(submitted).toMatchObject({
      name: 'Nova',
      settings: { additional: { workspaceRole: 'lead' } },
    });
    expect(submitted!.system).toContain('Preserve my original mission');
    expect(agents).toHaveLength(2);
    expect(helper).toEqual(original);
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBe(true);
  });
}
